//! Webhook route configuration schema (Phase 36.7.1, D-09).
//!
//! These types are the serde-facing CONFIGURATION contract for
//! `gateway.platforms.webhook.routes` in config.yaml. They live in
//! `ironhermes-core` — not in `ironhermes-restgw`, the crate that actually
//! implements the inbound adapter — because
//! [`crate::config::PlatformGatewayConfig`] must hold a `Vec<WebhookRoute>`
//! field and `ironhermes-restgw` depends on `ironhermes-core`; the reverse
//! dependency would form a cycle. `ironhermes_restgw::webhook::route_config`
//! re-exports these types rather than redefining them (Phase 36.7.1 Plan 01
//! Task 3's own action text identifies this dependency-direction
//! requirement).
//!
//! **Direction note:** this module is the schema for the INBOUND webhook
//! adapter (`ironhermes-restgw::webhook`). `ironhermes_hooks::webhook` is
//! the unrelated OUTBOUND hook-event delivery module — do not confuse the
//! two when reading call sites.

use serde::{Deserialize, Serialize};

/// Signature verification scheme selected per route (D-09). Enum dispatch,
/// not trait objects: the set is closed at four schemes for this phase, and
/// each variant is additive.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum SignatureKind {
    /// Timestamp-bound HMAC-SHA256 over `{timestamp}.{raw_body}`, carried in
    /// `X-Webhook-Signature-V2` + `X-Webhook-Timestamp` (both REQUIRED).
    /// D-10: this is the only generic scheme this phase ships — there is no
    /// legacy body-only unversioned fallback, and none is ever added.
    #[default]
    GenericV2,
    /// No verification. D-10: refused at `WebhookAdapter::new` construction
    /// time (not just at request time) unless the configured bind host is
    /// loopback — a misconfiguration fails to boot rather than quietly
    /// serving an unauthenticated agent endpoint to the network. There is no
    /// operator override.
    None,
    /// Twilio HMAC-SHA1-over-URL-plus-sorted-form-params, base64. Verifier
    /// arm implemented in Plan 02 — the variant is declared now because the
    /// enum shape (D-09) is the costly decision.
    Twilio,
    /// Telnyx Ed25519 signature. Verifier arm implemented in Plan 02.
    Telnyx,
}

/// Delivery target selector for a route's rendered answer (D-11/D-13). A
/// bare selector — the actual target value (URL string, platform name, chat
/// id) lives in the sibling fields on [`WebhookRoute`], matching the
/// `deliver` / `deliver_url` / `deliver_platform` / `deliver_chat_id`
/// config-key split in CONTEXT.md.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum DeliverTarget {
    /// Deliver back to the sender's own origin (a callback the payload
    /// names). Not implemented until a later plan in this phase — declared
    /// now for the enum shape.
    Origin,
    /// Deliver via a named platform adapter already configured on this
    /// gateway (e.g. `"telegram"`, `"buzz"`), resolved through the shared
    /// `DeliveryRegistry` handle.
    Platform,
    /// Deliver via an arbitrary URL. SSRF-checked at route-config load time
    /// AND again immediately before each delivery POST (D-11) — the double
    /// check is the documented mitigation for `is_safe_url`'s own
    /// DNS-rebinding TOCTOU limitation.
    #[default]
    Url,
}

/// Session identity mode for a route's per-delivery agent turn (O-03).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum SessionMode {
    /// Derive a fresh session identity (route name + a new v4 UUID) for
    /// every delivery, so two deliveries on the same route never share
    /// history. The safer default — an always-on route accumulating
    /// heterogeneous payloads is the shape a prior phase had to fix.
    #[default]
    Ephemeral,
    /// Reuse one persistent session identity across every delivery on this
    /// route. An explicit, auditable per-route opt-in, never an implicit
    /// global default.
    Persistent,
}

/// Env-indirected outbound auth header for `deliver: url` targets (D-14).
/// The credential VALUE is never written into config.yaml — only the name
/// of the environment variable it is read from at delivery time.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum OutboundAuth {
    /// No outbound auth header is attached.
    #[default]
    None,
    /// `Authorization: Bearer <value of $env>`.
    Bearer { env: String },
    /// HTTP Basic auth; user and password are each read from their own
    /// named environment variable.
    Basic { user_env: String, pass_env: String },
}

/// Per-route defensive limits (D-15): body-size cap enforced as a router
/// layer before the body is read, a fixed-window rate limiter, and the
/// idempotency cache TTL. Defaults match CONTEXT.md's stated numbers.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct RouteRails {
    /// Maximum accepted request body size, in bytes. Default 1 MiB.
    pub max_body_bytes: u64,
    /// Maximum accepted requests per minute, per route. Default 30.
    pub rate_limit_per_minute: u32,
    /// Idempotency cache entry lifetime, in seconds. Default 1 hour.
    pub idempotency_ttl_secs: u64,
}

impl Default for RouteRails {
    fn default() -> Self {
        Self {
            max_body_bytes: 1024 * 1024,
            rate_limit_per_minute: 30,
            idempotency_ttl_secs: 3600,
        }
    }
}

/// One configured inbound webhook route. The route collection is read once
/// at adapter construction from static config and owned immutably
/// thereafter — there is no mtime poll, no file watcher, no reload timer,
/// and no endpoint that mutates the route table at runtime (D-17).
///
/// Every field carries a default (via the struct-level `#[serde(default)]`
/// below), matching the 100%-consistent discipline already established on
/// [`crate::config::PlatformGatewayConfig`] — do not break it when adding a
/// field here either.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(default)]
pub struct WebhookRoute {
    /// Route identifier. Used as the `chat_id`/`sender_id` for the
    /// synthesized [`crate::MessageEvent`] and as the lookup key into the
    /// verifier map and the delivery configuration.
    pub name: String,
    /// The HTTP path this route answers on (e.g. `/webhook/my-route`).
    pub path: String,
    /// Which signature verification scheme this route uses (D-09/D-10).
    pub signature: SignatureKind,
    /// Name of the environment variable holding the shared HMAC secret for
    /// `signature: generic_v2`. The secret VALUE is never written to
    /// config.yaml (D-14).
    pub secret_env: Option<String>,
    /// Name of the environment variable holding the Twilio auth token, for
    /// `signature: twilio`.
    pub auth_token_env: Option<String>,
    /// Name of the environment variable holding the Telnyx Ed25519 public
    /// key, for `signature: telnyx`.
    pub public_key_env: Option<String>,
    /// Allowed absolute skew, in seconds, between `X-Webhook-Timestamp` and
    /// the verifier's clock. Default 300 (D-10).
    #[serde(default = "default_timestamp_skew_secs")]
    pub timestamp_skew_secs: u64,
    /// Brace-delimited placeholder template rendered against the inbound
    /// payload to build the agent's prompt (D-14 content-type-aware
    /// resolution — see `ironhermes_restgw::webhook::template`).
    pub prompt_template: String,
    /// Which [`DeliverTarget`] this route's rendered answer is sent to.
    pub deliver: DeliverTarget,
    /// Target URL, when `deliver: url`.
    pub deliver_url: Option<String>,
    /// Target platform name (e.g. `"telegram"`, `"buzz"`), when
    /// `deliver: platform`.
    pub deliver_platform: Option<String>,
    /// Target chat id on the platform named by `deliver_platform`. Falls
    /// back to this route's own `name` when absent.
    pub deliver_chat_id: Option<String>,
    /// D-13: when `true`, this route only delivers — it never runs an
    /// agent turn to produce content in the first place. Declared now for
    /// the schema; the routing behavior lands in a later plan.
    pub deliver_only: bool,
    /// Outbound auth header for `deliver: url` targets (D-14).
    pub outbound_auth: OutboundAuth,
    /// Session identity mode for this route's agent turns (O-03).
    pub session: SessionMode,
    /// Defensive limits — body size cap, rate limit, idempotency TTL
    /// (D-15).
    pub rails: RouteRails,
}

fn default_timestamp_skew_secs() -> u64 {
    300
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signature_kind_default_is_generic_v2() {
        assert_eq!(SignatureKind::default(), SignatureKind::GenericV2);
    }

    #[test]
    fn route_rails_defaults_match_spec() {
        let rails = RouteRails::default();
        assert_eq!(rails.max_body_bytes, 1024 * 1024);
        assert_eq!(rails.rate_limit_per_minute, 30);
        assert_eq!(rails.idempotency_ttl_secs, 3600);
    }

    #[test]
    fn webhook_route_deserializes_with_all_defaults_from_empty_object() {
        let route: WebhookRoute = serde_json::from_str("{}").expect("empty object deserializes");
        assert_eq!(route.signature, SignatureKind::GenericV2);
        assert_eq!(route.timestamp_skew_secs, 300);
        assert_eq!(route.rails.max_body_bytes, 1024 * 1024);
        assert_eq!(route.outbound_auth, OutboundAuth::None);
        assert_eq!(route.session, SessionMode::Ephemeral);
    }

    #[test]
    fn signature_kind_serializes_snake_case() {
        let json = serde_json::to_string(&SignatureKind::GenericV2).unwrap();
        assert_eq!(json, "\"generic_v2\"");
    }
}
