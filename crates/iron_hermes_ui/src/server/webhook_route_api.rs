//! Phase 49.3 Plan 04: webhook route CRUD over
//! `gateway.platforms["webhook"].routes: Vec<WebhookRoute>` (D-03/D-04).
//!
//! # The DTO is an explicit allowlist, never a passthrough (T-48.2-12-01 sibling)
//!
//! [`WebhookRouteView`] names exactly the fields the wizard/cards need —
//! mirroring `platform_config_api.rs`'s established DTO discipline. Every
//! field is a wasm-safe scalar (`String`/`bool`/`u64`/`u32`/`Option<...>`)
//! rather than a re-export of `ironhermes_core::webhook_route`'s typed
//! enums/structs, for TWO reasons: (1) the allowlist discipline itself —
//! a future secret-VALUE field added to the core struct cannot silently
//! start shipping toward the browser without a person deliberately
//! widening this allowlist AND updating
//! [`tests::webhook_route_view_dto_carries_no_secret_value_field`] below;
//! (2) `ironhermes-core` is declared ONLY under this crate's
//! `[target.'cfg(not(target_arch = "wasm32"))'.dependencies]` table
//! (`Cargo.toml:134-259`) — it does not exist on the wasm32 target at all,
//! so a `#[server]` fn's argument/return type (which the dioxus fullstack
//! macro must encode/decode on BOTH sides) can never name an
//! `ironhermes_core` type directly. `SignatureKind`/`DeliverTarget`/
//! `SessionMode` become plain lowercase-`snake_case` strings
//! (`"generic_v2"`/`"url"`/`"ephemeral"`, matching each enum's own serde
//! rename); `OutboundAuth`/`RouteRails` are flattened into scalar sibling
//! fields (`outbound_auth_kind` + `outbound_auth_env`/`_user_env`/
//! `_pass_env`; `rails_max_body_bytes`/`_rate_limit_per_minute`/
//! `_idempotency_ttl_secs`) rather than nested structs, for the same
//! wasm-visibility reason.
//!
//! # Route identity is the route's own `name` (D-04)
//!
//! CRUD keys on `WebhookRoute.name` — `upsert_webhook_route` replaces the
//! entry whose `name` matches the payload's `name`, or appends a new entry
//! when no match exists. Renaming an existing route's `name` therefore adds
//! a new entry rather than mutating the old one in place; this plan does
//! not build a rename affordance (D-04's "stable across edits" clause is
//! about the CARD-to-ROUTE mapping in the UI, not a server-side rename op).
//!
//! # The refusal predicate mirrors the adapter, never calls it (RESEARCH Anti-Pattern A3)
//!
//! [`route_would_refuse`] re-implements the ONE check the webhook adapter's
//! own construction-time constructor performs
//! (`crates/ironhermes-restgw/src/webhook/mod.rs:283`) — `signature: none`
//! combined with a non-loopback bind host. This module never constructs a
//! live adapter instance; the mirror exists purely so the wizard can warn
//! before a save that would leave the gateway refusing to boot. The
//! adapter's own check remains the sole server-side authority and is never
//! weakened by this mirror (T-49.3-04-02). It is deliberately defined over
//! [`WebhookRouteView`] (a plain-scalar `signature: String` field), not
//! over `ironhermes_core::webhook_route::WebhookRoute` — see this module's
//! doc "The DTO is an explicit allowlist" section for why: the wizard calls
//! this predicate directly on the wasm CLIENT, with no server round trip,
//! and `WebhookRoute` does not exist on that target at all.

use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

use crate::server::tools_config_api::ConfigScope;

/// The `gateway.platforms` map key this module reads and writes. Never
/// hardcoded a second time below.
#[cfg(not(target_arch = "wasm32"))]
const WEBHOOK_PLATFORM_KEY: &str = "webhook";

// =============================================================================
// DTOs — shared shape on both the wasm client and the native server.
// =============================================================================

/// Explicit field allowlist over one `WebhookRoute` entry — see module doc's
/// "The DTO is an explicit allowlist" section. Also doubles as the
/// upsert-write payload (create/edit share one shape, matching the wizard's
/// "existing routes open in the same form" contract, D-03). Every field is
/// a wasm-safe scalar — see module doc for why nested `ironhermes_core`
/// enums/structs are never reused here directly.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct WebhookRouteView {
    pub name: String,
    pub path: String,
    /// `"generic_v2"` | `"none"` | `"twilio"` | `"telnyx"` — matches
    /// `SignatureKind`'s own `#[serde(rename_all = "snake_case")]` spelling.
    pub signature: String,
    /// Env var NAME holding the `generic_v2` HMAC secret — never the value.
    pub secret_env: Option<String>,
    /// Env var NAME holding the Twilio auth token — never the value.
    pub auth_token_env: Option<String>,
    /// Env var NAME holding the Telnyx Ed25519 public key — never the value.
    pub public_key_env: Option<String>,
    pub timestamp_skew_secs: u64,
    pub prompt_template: String,
    /// `"url"` | `"origin"` | `"platform"`.
    pub deliver: String,
    pub deliver_url: Option<String>,
    pub deliver_platform: Option<String>,
    pub deliver_chat_id: Option<String>,
    pub deliver_only: bool,
    /// `"none"` | `"bearer"` | `"basic"` — the `OutboundAuth` variant tag.
    /// Env-NAME-only outbound auth reference — never a credential value.
    pub outbound_auth_kind: String,
    /// `Bearer`'s env var NAME (present only when `outbound_auth_kind ==
    /// "bearer"`).
    pub outbound_auth_env: Option<String>,
    /// `Basic`'s username env var NAME (present only when
    /// `outbound_auth_kind == "basic"`).
    pub outbound_auth_user_env: Option<String>,
    /// `Basic`'s password env var NAME (present only when
    /// `outbound_auth_kind == "basic"`).
    pub outbound_auth_pass_env: Option<String>,
    /// `"ephemeral"` | `"persistent"`.
    pub session: String,
    pub rails_max_body_bytes: u64,
    pub rails_rate_limit_per_minute: u32,
    pub rails_idempotency_ttl_secs: u64,
}

impl WebhookRouteView {
    /// Convert a stored `WebhookRoute` (no secret-bearing field at any
    /// depth) into the browser-facing DTO. Field-by-field, never a
    /// passthrough struct-update — see module doc. Native-only: it names
    /// `ironhermes_core::webhook_route::WebhookRoute`, a type that does not
    /// exist on the wasm32 target (module doc).
    #[cfg(not(target_arch = "wasm32"))]
    fn from_route(route: &ironhermes_core::webhook_route::WebhookRoute) -> Self {
        use ironhermes_core::webhook_route::{DeliverTarget, OutboundAuth, SessionMode, SignatureKind};

        let signature = match route.signature {
            SignatureKind::GenericV2 => "generic_v2",
            SignatureKind::None => "none",
            SignatureKind::Twilio => "twilio",
            SignatureKind::Telnyx => "telnyx",
        }
        .to_string();
        let deliver = match route.deliver {
            DeliverTarget::Url => "url",
            DeliverTarget::Origin => "origin",
            DeliverTarget::Platform => "platform",
        }
        .to_string();
        let session = match route.session {
            SessionMode::Ephemeral => "ephemeral",
            SessionMode::Persistent => "persistent",
        }
        .to_string();
        let (outbound_auth_kind, outbound_auth_env, outbound_auth_user_env, outbound_auth_pass_env) =
            match &route.outbound_auth {
                OutboundAuth::None => ("none".to_string(), None, None, None),
                OutboundAuth::Bearer { env } => ("bearer".to_string(), Some(env.clone()), None, None),
                OutboundAuth::Basic { user_env, pass_env } => (
                    "basic".to_string(),
                    None,
                    Some(user_env.clone()),
                    Some(pass_env.clone()),
                ),
            };

        Self {
            name: route.name.clone(),
            path: route.path.clone(),
            signature,
            secret_env: route.secret_env.clone(),
            auth_token_env: route.auth_token_env.clone(),
            public_key_env: route.public_key_env.clone(),
            timestamp_skew_secs: route.timestamp_skew_secs,
            prompt_template: route.prompt_template.clone(),
            deliver,
            deliver_url: route.deliver_url.clone(),
            deliver_platform: route.deliver_platform.clone(),
            deliver_chat_id: route.deliver_chat_id.clone(),
            deliver_only: route.deliver_only,
            outbound_auth_kind,
            outbound_auth_env,
            outbound_auth_user_env,
            outbound_auth_pass_env,
            session,
            rails_max_body_bytes: route.rails.max_body_bytes,
            rails_rate_limit_per_minute: route.rails.rate_limit_per_minute,
            rails_idempotency_ttl_secs: route.rails.idempotency_ttl_secs,
        }
    }

    /// Convert the write payload back into the core `WebhookRoute` shape
    /// that is actually persisted to `config.yaml`. The inverse of
    /// [`Self::from_route`] — every field maps 1:1, no field is dropped or
    /// defaulted away. Native-only for the same reason as
    /// [`Self::from_route`]. An unrecognized string in any tagged field
    /// (`signature`/`deliver`/`session`/`outbound_auth_kind`) falls back to
    /// that type's own `Default` variant — a total-function safety net, NOT
    /// a validation strategy (WR-05 revision: the prior doc here reasoned
    /// "the client always constructs valid values," which the `#[server]`
    /// fn's real threat model disputes — it is a genuine HTTP endpoint an
    /// authenticated session can call with a hand-crafted payload, e.g. via
    /// [`parse_pasted_route`]'s JSON/YAML path). [`validate_route_fields`]
    /// is the check that makes these fallbacks UNREACHABLE for any payload
    /// that has passed validation — call it first, always.
    #[cfg(not(target_arch = "wasm32"))]
    fn into_route(self) -> ironhermes_core::webhook_route::WebhookRoute {
        use ironhermes_core::webhook_route::{
            DeliverTarget, OutboundAuth, RouteRails, SessionMode, SignatureKind, WebhookRoute,
        };

        let signature = match self.signature.as_str() {
            "none" => SignatureKind::None,
            "twilio" => SignatureKind::Twilio,
            "telnyx" => SignatureKind::Telnyx,
            _ => SignatureKind::GenericV2,
        };
        let deliver = match self.deliver.as_str() {
            "origin" => DeliverTarget::Origin,
            "platform" => DeliverTarget::Platform,
            _ => DeliverTarget::Url,
        };
        let session = match self.session.as_str() {
            "persistent" => SessionMode::Persistent,
            _ => SessionMode::Ephemeral,
        };
        let outbound_auth = match self.outbound_auth_kind.as_str() {
            "bearer" => OutboundAuth::Bearer {
                env: self.outbound_auth_env.unwrap_or_default(),
            },
            "basic" => OutboundAuth::Basic {
                user_env: self.outbound_auth_user_env.unwrap_or_default(),
                pass_env: self.outbound_auth_pass_env.unwrap_or_default(),
            },
            _ => OutboundAuth::None,
        };

        WebhookRoute {
            name: self.name,
            path: self.path,
            signature,
            secret_env: self.secret_env,
            auth_token_env: self.auth_token_env,
            public_key_env: self.public_key_env,
            timestamp_skew_secs: self.timestamp_skew_secs,
            prompt_template: self.prompt_template,
            deliver,
            deliver_url: self.deliver_url,
            deliver_platform: self.deliver_platform,
            deliver_chat_id: self.deliver_chat_id,
            deliver_only: self.deliver_only,
            outbound_auth,
            session,
            rails: RouteRails {
                max_body_bytes: self.rails_max_body_bytes,
                rate_limit_per_minute: self.rails_rate_limit_per_minute,
                idempotency_ttl_secs: self.rails_idempotency_ttl_secs,
            },
        }
    }
}

// =============================================================================
// Server-only helpers — pure where possible (mirrors
// `platform_config_api.rs`'s test-reachability discipline).
// =============================================================================

/// D-10 sibling of `platform_config_api::check_buzz_write_gate` /
/// `gateway_env_secret_api::check_gateway_write_gate` — the established
/// per-module duplication pattern in this phase (each new server-fn module
/// carries its own copy rather than reaching into a sibling module's
/// private fn). Fail-closed: reads `security.web_config_write_enabled` from
/// a FRESH ROOT `Config::load()` regardless of the scope being edited.
#[cfg(not(target_arch = "wasm32"))]
fn check_webhook_write_gate() -> Result<(), String> {
    let root_config =
        ironhermes_core::config::Config::load().map_err(|e| format!("Config load failed: {e}"))?;
    if !root_config.security.web_config_write_enabled {
        return Err("Config writes are disabled".to_string());
    }
    Ok(())
}

/// Validate every env-NAME field a route carries (`secret_env`,
/// `auth_token_env`, `public_key_env`, and the outbound-auth env fields)
/// against `[A-Z][A-Z0-9_]*` — reuses `profile_api::validate_key_name`
/// (T-49.3-04-04), never a re-implemented pattern. Concatenates every
/// problem found rather than stopping at the first, mirroring
/// `platform_config_api::validate_and_normalize_entries`'s "an operator
/// with two typos sees both in one round trip" precedent.
#[cfg(not(target_arch = "wasm32"))]
fn validate_route_env_names(route: &WebhookRouteView) -> Result<(), Vec<String>> {
    let mut errors = Vec::new();
    let mut check = |label: &str, value: &Option<String>| {
        if let Some(name) = value {
            if let Err(e) = crate::server::profile_api::validate_key_name(name) {
                errors.push(format!("{label}: {e}"));
            }
        }
    };
    check("secret_env", &route.secret_env);
    check("auth_token_env", &route.auth_token_env);
    check("public_key_env", &route.public_key_env);
    match route.outbound_auth_kind.as_str() {
        "bearer" => check("outbound_auth.env", &route.outbound_auth_env),
        "basic" => {
            check("outbound_auth.user_env", &route.outbound_auth_user_env);
            check("outbound_auth.pass_env", &route.outbound_auth_pass_env);
        }
        _ => {}
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

// -------------------------------------------------------------------
// Overwrite/validation bounds (CR-01 / WR-01) — named consts, not
// inline magic numbers.
// -------------------------------------------------------------------

#[cfg(not(target_arch = "wasm32"))]
const MAX_ROUTE_NAME_LEN: usize = 64;
#[cfg(not(target_arch = "wasm32"))]
const MAX_ROUTE_PATH_LEN: usize = 256;
#[cfg(not(target_arch = "wasm32"))]
const MAX_PROMPT_TEMPLATE_LEN: usize = 8192;
#[cfg(not(target_arch = "wasm32"))]
const MAX_DELIVER_URL_LEN: usize = 2048;
#[cfg(not(target_arch = "wasm32"))]
const MAX_DELIVER_PLATFORM_LEN: usize = 64;
#[cfg(not(target_arch = "wasm32"))]
const MAX_DELIVER_CHAT_ID_LEN: usize = 128;

/// De-collide a candidate route name against `existing` names in the same
/// scope — the second half of `49.3-VERIFICATION.md`'s `missing:` list
/// (CR-01): re-selecting a preset tile must not collide by default even if
/// the operator dismisses every dialog. Returns `base` unchanged when it is
/// absent from `existing`; otherwise the first free `"{base}-{n}"` for `n`
/// in `2..=999`. If every candidate in that range is somehow already taken,
/// returns `base` unchanged — the REPLACE ROUTE confirm (client half, Task
/// 2) plus [`upsert_webhook_route_impl`]'s unconfirmed-collision refusal
/// are the backstop in that exhausted-range case; this helper is a
/// convenience, never the guard.
#[cfg(not(target_arch = "wasm32"))]
fn unique_route_name(base: &str, existing: &[String]) -> String {
    if !existing.iter().any(|n| n == base) {
        return base.to_string();
    }
    for n in 2..=999u32 {
        let candidate = format!("{base}-{n}");
        if !existing.iter().any(|n| n == &candidate) {
            return candidate;
        }
    }
    base.to_string()
}

/// Track a preset's `path` alongside a de-collided `unique_name`, so a
/// mis-clicked preset tile arrives pre-named AND pre-pathed. Only rewrites
/// the path when it still has the `/webhook/{original_name}` shape every
/// preset constructor above uses — this conditional keeps the helper
/// honest if a future preset's path ever stops tracking its name, rather
/// than mangling an unrelated, hand-edited path.
#[cfg(not(target_arch = "wasm32"))]
fn derive_preset_path(original_path: &str, original_name: &str, unique_name: &str) -> String {
    if original_path == format!("/webhook/{original_name}") {
        format!("/webhook/{unique_name}")
    } else {
        original_path.to_string()
    }
}

/// The vocabulary each tag-shaped `WebhookRouteView` field is allowed to
/// carry — EXACTLY the option values the wizard's `<select>` elements emit
/// (`webhook_wizard.rs`'s `RouteEditorModal`) and EXACTLY the strings
/// [`WebhookRouteView::from_route`] produces (WR-05). Named consts, not
/// inline literals repeated at each check site and in tests.
#[cfg(not(target_arch = "wasm32"))]
const ALLOWED_SIGNATURES: [&str; 4] = ["generic_v2", "none", "twilio", "telnyx"];
#[cfg(not(target_arch = "wasm32"))]
const ALLOWED_DELIVER_TARGETS: [&str; 3] = ["url", "origin", "platform"];
#[cfg(not(target_arch = "wasm32"))]
const ALLOWED_SESSIONS: [&str; 2] = ["ephemeral", "persistent"];
#[cfg(not(target_arch = "wasm32"))]
const ALLOWED_OUTBOUND_AUTH_KINDS: [&str; 3] = ["none", "bearer", "basic"];

/// WR-01/WR-05: mirror `WebhookAdapter::new`'s (`ironhermes-restgw/src/
/// webhook/mod.rs`) construction-time path rules — leading slash required,
/// no capture segment — plus length/control-character bounds on every
/// field, as an EARLIER refusal than the adapter's own, PLUS (WR-05) a
/// vocabulary check on the four tag-shaped fields (`signature`/`deliver`/
/// `session`/`outbound_auth_kind`) so [`WebhookRouteView::into_route`]'s
/// default-variant fallbacks become unreachable for any payload that passes
/// here — see `into_route`'s doc for why silent coercion (the pre-WR-05
/// behavior) is a real, not theoretical, risk: an unrecognized
/// `outbound_auth_kind` used to silently become `OutboundAuth::None`,
/// dropping a configured outbound Authorization header with no error at
/// all. The adapter remains the sole server-side authority for the
/// path/field-bound checks: this mirror only ever refuses MORE than the
/// adapter, never less, so it can never let a route through that the
/// adapter would reject. Every message names the offending FIELD and the
/// bound it violated and NEVER interpolates the field's own value
/// (T-48.2-12-10) — an over-length, control-character-laden, or
/// out-of-vocabulary value must never be echoed back into the error
/// channel. Accumulates every problem into one `Vec`, mirroring
/// [`validate_route_env_names`]'s "an operator with two typos sees both in
/// one round trip" precedent.
#[cfg(not(target_arch = "wasm32"))]
fn validate_route_fields(route: &WebhookRouteView) -> Result<(), Vec<String>> {
    let mut errors = Vec::new();

    if route.name.trim().is_empty() {
        errors.push("name must not be empty or whitespace-only".to_string());
    } else if route.name.len() > MAX_ROUTE_NAME_LEN {
        errors.push(format!("name exceeds {MAX_ROUTE_NAME_LEN} characters"));
    } else if route.name.chars().any(char::is_control) {
        errors.push("name must not contain a control character".to_string());
    }

    if route.path.trim().is_empty() {
        errors.push("path must not be empty or whitespace-only".to_string());
    } else if route.path.len() > MAX_ROUTE_PATH_LEN {
        errors.push(format!("path exceeds {MAX_ROUTE_PATH_LEN} characters"));
    } else if route
        .path
        .chars()
        .any(|c| char::is_control(c) || c.is_whitespace())
    {
        errors.push("path must not contain a control character or whitespace".to_string());
    } else if !route.path.starts_with('/') {
        errors.push("path must start with '/'".to_string());
    } else if route
        .path
        .split('/')
        .any(|seg| seg.starts_with(':') || seg.starts_with('{'))
    {
        errors.push("path must not contain a capture segment (a ':' or '{' prefixed segment)".to_string());
    }

    if route.prompt_template.len() > MAX_PROMPT_TEMPLATE_LEN {
        errors.push(format!(
            "prompt_template exceeds {MAX_PROMPT_TEMPLATE_LEN} characters"
        ));
    }

    if let Some(url) = &route.deliver_url {
        if url.len() > MAX_DELIVER_URL_LEN {
            errors.push(format!("deliver_url exceeds {MAX_DELIVER_URL_LEN} characters"));
        } else if url.chars().any(char::is_control) {
            errors.push("deliver_url must not contain a control character".to_string());
        }
    }
    if let Some(platform) = &route.deliver_platform {
        if platform.len() > MAX_DELIVER_PLATFORM_LEN {
            errors.push(format!(
                "deliver_platform exceeds {MAX_DELIVER_PLATFORM_LEN} characters"
            ));
        } else if platform.chars().any(char::is_control) {
            errors.push("deliver_platform must not contain a control character".to_string());
        }
    }
    if let Some(chat_id) = &route.deliver_chat_id {
        if chat_id.len() > MAX_DELIVER_CHAT_ID_LEN {
            errors.push(format!(
                "deliver_chat_id exceeds {MAX_DELIVER_CHAT_ID_LEN} characters"
            ));
        } else if chat_id.chars().any(char::is_control) {
            errors.push("deliver_chat_id must not contain a control character".to_string());
        }
    }

    // WR-05: the four tag-shaped fields are plain `String`s on the wire
    // (module doc's rationale for why `ironhermes_core`'s typed enums
    // aren't reused directly) — the rendered `<select>` UI constrains them
    // in ordinary use, but the `#[server]` fn is a real endpoint an
    // authenticated session can call directly with an arbitrary payload.
    // Accumulate, never early-return, matching every other check above.
    if !ALLOWED_SIGNATURES.contains(&route.signature.as_str()) {
        errors.push(format!(
            "signature must be one of: {}",
            ALLOWED_SIGNATURES.join(", ")
        ));
    }
    if !ALLOWED_DELIVER_TARGETS.contains(&route.deliver.as_str()) {
        errors.push(format!(
            "deliver must be one of: {}",
            ALLOWED_DELIVER_TARGETS.join(", ")
        ));
    }
    if !ALLOWED_SESSIONS.contains(&route.session.as_str()) {
        errors.push(format!(
            "session must be one of: {}",
            ALLOWED_SESSIONS.join(", ")
        ));
    }
    if !ALLOWED_OUTBOUND_AUTH_KINDS.contains(&route.outbound_auth_kind.as_str()) {
        errors.push(format!(
            "outbound_auth_kind must be one of: {}",
            ALLOWED_OUTBOUND_AUTH_KINDS.join(", ")
        ));
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

/// Pure predicate mirroring the ONE construction-time refusal the webhook
/// adapter's own constructor performs for `signature: none`
/// (`ironhermes-restgw/src/webhook/mod.rs:283`) — see module doc's
/// "refusal predicate mirrors the adapter" section. Unconditional (no
/// `wasm32` gate) — the wizard calls this directly on the wasm client, no
/// round trip. `bind_host` that fails to parse as an IP address is treated
/// as "would refuse" (conservative): the real adapter also refuses
/// construction on an unparseable host (a DIFFERENT error, "invalid bind
/// host"), so this predicate never tells the wizard a route is safe when
/// the adapter cannot even determine loopback status.
#[allow(dead_code)] // consumed from cfg-gated UI call sites; dead_code fires under --all-features (mutually-exclusive renderer features)
pub fn route_would_refuse(route: &WebhookRouteView, bind_host: &str) -> bool {
    if route.signature != "none" {
        return false;
    }
    match bind_host.parse::<std::net::IpAddr>() {
        Ok(ip) => !ip.is_loopback(),
        Err(_) => true,
    }
}

/// Deserialize a pasted drop-in route snippet (D-03's PASTE ROUTE CONFIG
/// path) — tries JSON first, then YAML, since either is a plausible paste
/// source and `WebhookRoute`'s `#[serde(default)]` means a bare `{}` (or an
/// empty/whitespace-only YAML mapping) deserializes successfully with every
/// field defaulted (matches `WEBHOOK-AND-REST-API.md`'s "a bare `{}` route
/// entry deserializes successfully" contract). Native-only — `serde_yaml`
/// is ALSO declared only under this crate's wasm32-excluded dependency
/// table (module doc); exposed to the wasm client only via the
/// [`parse_pasted_route`] `#[server]` fn below.
#[cfg(not(target_arch = "wasm32"))]
fn parse_route_snippet(text: &str) -> Result<ironhermes_core::webhook_route::WebhookRoute, String> {
    if let Ok(route) = serde_json::from_str(text) {
        return Ok(route);
    }
    serde_yaml::from_str(text).map_err(|e| format!("could not parse route config: {e}"))
}

// =============================================================================
// Preset constructors (D-03) — build `WebhookRoute` directly, matching the
// three worked examples in `WEBHOOK-AND-REST-API.md`. No parallel route
// shape is invented (49.3-PATTERNS.md). Native-only (module doc) — exposed
// to the wasm client only via the [`preset_webhook_route`] `#[server]` fn
// below, which converts to [`WebhookRouteView`] before crossing the wire.
// =============================================================================

/// Twilio SMS (CPaaS inbound) preset — signature `twilio`, inbound-only
/// (D-01/D-02: no outbound/reply-by-SMS affordance is ever built from this
/// preset), delivering to a named platform chat.
#[cfg(not(target_arch = "wasm32"))]
fn twilio_sms_preset() -> ironhermes_core::webhook_route::WebhookRoute {
    use ironhermes_core::webhook_route::{DeliverTarget, SignatureKind, WebhookRoute};
    WebhookRoute {
        name: "sms-inbound".to_string(),
        path: "/webhook/sms-inbound".to_string(),
        signature: SignatureKind::Twilio,
        auth_token_env: Some("TWILIO_AUTH_TOKEN".to_string()),
        prompt_template: "SMS from {From}: {Body}".to_string(),
        deliver: DeliverTarget::Platform,
        deliver_platform: Some("telegram".to_string()),
        deliver_chat_id: Some("123456789".to_string()),
        ..Default::default()
    }
}

/// n8n / generic automation-tool round trip preset — signature
/// `generic_v2`, delivers to an arbitrary callback URL with a bearer token.
#[cfg(not(target_arch = "wasm32"))]
fn n8n_generic_preset() -> ironhermes_core::webhook_route::WebhookRoute {
    use ironhermes_core::webhook_route::{DeliverTarget, OutboundAuth, SignatureKind, WebhookRoute};
    WebhookRoute {
        name: "n8n-trigger".to_string(),
        path: "/webhook/n8n-trigger".to_string(),
        signature: SignatureKind::GenericV2,
        secret_env: Some("N8N_WEBHOOK_SECRET".to_string()),
        prompt_template: "Automation event: {event_type} — {summary}".to_string(),
        deliver: DeliverTarget::Url,
        deliver_url: Some("https://n8n.example.com/webhook/callback".to_string()),
        outbound_auth: OutboundAuth::Bearer {
            env: "N8N_CALLBACK_TOKEN".to_string(),
        },
        ..Default::default()
    }
}

/// CRM deliver-only round trip preset (Twenty CRM-shaped) — `deliver_only:
/// true` (no agent turn runs at all), posts the rendered note to the CRM's
/// API using HTTP Basic auth.
#[cfg(not(target_arch = "wasm32"))]
fn crm_deliver_only_preset() -> ironhermes_core::webhook_route::WebhookRoute {
    use ironhermes_core::webhook_route::{DeliverTarget, OutboundAuth, SignatureKind, WebhookRoute};
    WebhookRoute {
        name: "crm-update".to_string(),
        path: "/webhook/crm-update".to_string(),
        signature: SignatureKind::GenericV2,
        secret_env: Some("TWENTY_CRM_WEBHOOK_SECRET".to_string()),
        prompt_template: "CRM record updated: {record_name} ({record_id})".to_string(),
        deliver_only: true,
        deliver: DeliverTarget::Url,
        deliver_url: Some("https://twentycrm.example.com/api/notes".to_string()),
        outbound_auth: OutboundAuth::Basic {
            user_env: "TWENTY_CRM_API_USER".to_string(),
            pass_env: "TWENTY_CRM_API_PASSWORD".to_string(),
        },
        ..Default::default()
    }
}

// =============================================================================
// CRUD core — pure(-ish), disk-backed. Mirrors
// `platform_config_api::set_buzz_edit_impl`'s staged-write order: resolve
// scope (fresh disk read) -> gate check -> read-modify-write the routes Vec
// -> atomic save -> re-read fresh from disk.
// =============================================================================

#[cfg(not(target_arch = "wasm32"))]
fn list_webhook_routes_impl(scope: &ConfigScope) -> Result<Vec<WebhookRouteView>, String> {
    let (config, _target) = crate::server::tools_config_api::resolve_scope_target(scope)?;
    Ok(config
        .gateway
        .platforms
        .get(WEBHOOK_PLATFORM_KEY)
        .map(|p| p.routes.iter().map(WebhookRouteView::from_route).collect())
        .unwrap_or_default())
}

#[cfg(not(target_arch = "wasm32"))]
fn get_webhook_route_impl(
    scope: &ConfigScope,
    name: &str,
) -> Result<Option<WebhookRouteView>, String> {
    let (config, _target) = crate::server::tools_config_api::resolve_scope_target(scope)?;
    Ok(config
        .gateway
        .platforms
        .get(WEBHOOK_PLATFORM_KEY)
        .and_then(|p| p.routes.iter().find(|r| r.name == name))
        .map(WebhookRouteView::from_route))
}

/// Read the webhook listener's OWN configured bind host
/// (`gateway.platforms["webhook"].host`) — needed by the wizard's
/// client-side [`route_would_refuse`] mirror (T-49.3-04-02), which must
/// evaluate against the REAL bind host, not a guess. `None` when the
/// `webhook:` block or its `host` field is absent — WEBHOOK-AND-REST-API.md
/// documents no default bind host, so absence is a real, distinct answer
/// (mirrors `build_buzz_view`'s "'not configured' is its own answer"
/// discipline), never coerced to a fake loopback/non-loopback guess.
#[cfg(not(target_arch = "wasm32"))]
fn get_webhook_bind_host_impl(scope: &ConfigScope) -> Result<Option<String>, String> {
    let (config, _target) = crate::server::tools_config_api::resolve_scope_target(scope)?;
    Ok(config
        .gateway
        .platforms
        .get(WEBHOOK_PLATFORM_KEY)
        .and_then(|p| p.host.clone()))
}

/// Read the webhook listener's configured bind host for `scope` — see
/// [`get_webhook_bind_host_impl`].
#[server]
pub async fn get_webhook_bind_host(scope: ConfigScope) -> Result<Option<String>, ServerFnError> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        get_webhook_bind_host_impl(&scope).map_err(ServerFnError::new)
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = scope;
        unreachable!("server fn body never runs on the wasm client")
    }
}

/// Upsert order (CR-01/WR-01/CR-02, revised): (1) run BOTH
/// [`validate_route_fields`] and [`validate_route_env_names`], concatenating
/// their error `Vec`s so an operator with a bad path AND a bad env name
/// sees both in one round trip — no disk I/O yet, a rejected field aborts
/// here; (2) resolve scope (fresh disk read); (3) gate check
/// ([`check_webhook_write_gate`], unchanged, still first among the gates);
/// (4) collision guard — an unconfirmed name collision (`allow_overwrite ==
/// false`) is refused INSIDE this same read-modify-write, EXCEPT the route
/// named by `editing_name` (CR-02: the route being updated in place is not
/// a collision with itself) — a client whose route list went stale between
/// fetch and save still cannot race past it; (5) duplicate-path guard —
/// mirrors the adapter's `seen_paths` refusal at an earlier, recoverable
/// moment, exempting the SAME `editing_name` route so a rename that keeps
/// its own path is not refused; (6) NEW (CR-02) rename removal — when
/// `editing_name` is present and differs from the payload's `name`, drop
/// the entry named `editing_name` from the routes Vec BEFORE the
/// replace-or-append match below, so a rename MOVES the route instead of
/// appending a second entry and orphaning the original (D-04: one card per
/// route, keyed by name); (7) read-modify-write the routes Vec (replace by
/// `name` if present, else append, D-04) -> atomic save -> re-read fresh
/// from disk so the returned DTO reflects what is actually on disk.
///
/// `editing_name` is the name the route was opened under — `None` when the
/// caller is creating a new route. It is a client-supplied identity
/// ASSERTION, not proof: it narrows the collision/duplicate-path guards and
/// drives the rename removal, but grants no authority the caller lacked —
/// the same authenticated session could already pass `allow_overwrite:
/// true` directly, or call [`delete_webhook_route_impl`] outright. It
/// exists because D-04 keys a route on its `name` alone, so without an
/// explicit signal the server cannot distinguish "this route, updated" from
/// "a different route that already owns this name" (CR-02).
#[cfg(not(target_arch = "wasm32"))]
async fn upsert_webhook_route_impl(
    scope: ConfigScope,
    payload: WebhookRouteView,
    allow_overwrite: bool,
    editing_name: Option<String>,
) -> Result<WebhookRouteView, Vec<String>> {
    let mut errors = Vec::new();
    if let Err(field_errors) = validate_route_fields(&payload) {
        errors.extend(field_errors);
    }
    if let Err(env_errors) = validate_route_env_names(&payload) {
        errors.extend(env_errors);
    }
    if !errors.is_empty() {
        return Err(errors);
    }

    let (mut config, target) =
        crate::server::tools_config_api::resolve_scope_target(&scope).map_err(|e| vec![e])?;
    check_webhook_write_gate().map_err(|e| vec![e])?;

    let name = payload.name.clone();
    let mut platform = config
        .gateway
        .platforms
        .get(WEBHOOK_PLATFORM_KEY)
        .cloned()
        .unwrap_or_default();

    // CR-01/CR-02: an unconfirmed name collision is refused INSIDE the same
    // read-modify-write that would perform the destructive replace, so a
    // client whose route list went stale between fetch and save cannot
    // race past this guard — EXCEPT the route `editing_name` names, which
    // is not a collision with itself (the case CR-02 broke: an in-place
    // edit resubmits its own unchanged name). `RouteEditorModal`'s REPLACE
    // ROUTE confirm (Task 2) is the operator-facing half; this is the
    // authority.
    let collides_by_name = platform
        .routes
        .iter()
        .any(|r| r.name == name && editing_name.as_deref() != Some(r.name.as_str()));
    if collides_by_name && !allow_overwrite {
        return Err(vec![format!(
            "a route named '{name}' already exists — confirming the replacement is required"
        )]);
    }

    // T-49.3-07-03/CR-02: mirrors the adapter's `seen_paths` refusal
    // (`ironhermes-restgw/src/webhook/mod.rs`) at an earlier, recoverable
    // moment. Only an OTHER route (a different name, and not the route
    // named by `editing_name`) can collide here — replacing THIS name's own
    // existing path, or a rename that keeps its own path, is not a
    // collision.
    if platform.routes.iter().any(|r| {
        r.name != name && editing_name.as_deref() != Some(r.name.as_str()) && r.path == payload.path
    }) {
        return Err(vec!["path is already used by another route".to_string()]);
    }

    // CR-02: rename removal. When `editing_name` names a DIFFERENT route
    // than the payload's own `name`, this save is a rename — drop the old
    // entry BEFORE the replace-or-append match below, so the rename MOVES
    // the route (D-04: one card per route, keyed by name) instead of
    // appending a second entry and orphaning the original under its old
    // name (the failure the review documented parenthetically inside
    // CR-02).
    if let Some(old_name) = editing_name.as_deref() {
        if old_name != name {
            platform.routes.retain(|r| r.name != old_name);
        }
    }

    let route = payload.into_route();
    match platform.routes.iter_mut().find(|r| r.name == name) {
        Some(existing) => *existing = route,
        None => platform.routes.push(route),
    }
    config
        .gateway
        .platforms
        .insert(WEBHOOK_PLATFORM_KEY.to_string(), platform);
    crate::server::tools_config_api::save_scoped(&config, &target).map_err(|e| vec![e])?;

    let (reread, _reread_target) =
        crate::server::tools_config_api::resolve_scope_target(&scope).map_err(|e| vec![e])?;
    reread
        .gateway
        .platforms
        .get(WEBHOOK_PLATFORM_KEY)
        .and_then(|p| p.routes.iter().find(|r| r.name == name))
        .map(WebhookRouteView::from_route)
        .ok_or_else(|| vec!["route vanished immediately after being saved".to_string()])
}

/// Delete order: resolve scope (fresh disk read) -> gate check -> remove
/// the matching entry (no-op, not an error, when the name is already
/// absent — mirrors the idempotent-delete precedent elsewhere in this
/// crate) -> atomic save.
#[cfg(not(target_arch = "wasm32"))]
async fn delete_webhook_route_impl(scope: ConfigScope, name: String) -> Result<(), String> {
    let (mut config, target) = crate::server::tools_config_api::resolve_scope_target(&scope)?;
    check_webhook_write_gate()?;

    if let Some(platform) = config.gateway.platforms.get_mut(WEBHOOK_PLATFORM_KEY) {
        platform.routes.retain(|r| r.name != name);
    }
    crate::server::tools_config_api::save_scoped(&config, &target)?;
    Ok(())
}

// =============================================================================
// #[server] fns — thin wrappers, dioxus fullstack codec split.
// =============================================================================

/// List every configured webhook route for `scope` — an absent `webhook:`
/// block or an absent `routes:` key both yield an empty `Vec`, never an
/// error.
#[server]
pub async fn list_webhook_routes(scope: ConfigScope) -> Result<Vec<WebhookRouteView>, ServerFnError> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        list_webhook_routes_impl(&scope).map_err(ServerFnError::new)
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = scope;
        unreachable!("server fn body never runs on the wasm client")
    }
}

/// Resolve one route by `name` for `scope` — `None` when no route with that
/// name exists (never an error).
#[server]
pub async fn get_webhook_route(
    scope: ConfigScope,
    name: String,
) -> Result<Option<WebhookRouteView>, ServerFnError> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        get_webhook_route_impl(&scope, &name).map_err(ServerFnError::new)
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (scope, name);
        unreachable!("server fn body never runs on the wasm client")
    }
}

/// Native body of [`preset_webhook_route`] — separated for direct native
/// test reachability, matching this module's established `_impl` pattern.
/// Resolves the named preset, then de-collides its name (and the path that
/// tracks it) against `scope`'s CURRENTLY configured route names via
/// [`unique_route_name`]/[`derive_preset_path`]. On a failed list read the
/// existing-names list is treated as EMPTY — building a draft is not a
/// write, and [`upsert_webhook_route_impl`]'s collision guard still
/// protects the actual save, so a failed read here must never block the
/// wizard. `parse_pasted_route` is deliberately NOT changed the same way: a
/// pasted snippet carries an operator-authored name, and silently renaming
/// it would be a worse surprise than the REPLACE ROUTE confirm (Task 2).
#[cfg(not(target_arch = "wasm32"))]
fn preset_webhook_route_impl(kind: &str, scope: &ConfigScope) -> Result<WebhookRouteView, String> {
    let route = match kind {
        "twilio" => twilio_sms_preset(),
        "n8n" => n8n_generic_preset(),
        "crm" => crm_deliver_only_preset(),
        other => return Err(format!("unknown preset kind '{other}'")),
    };
    let mut view = WebhookRouteView::from_route(&route);
    let existing_names: Vec<String> = list_webhook_routes_impl(scope)
        .map(|routes| routes.into_iter().map(|r| r.name).collect())
        .unwrap_or_default();
    let unique_name = unique_route_name(&view.name, &existing_names);
    if unique_name != view.name {
        view.path = derive_preset_path(&view.path, &view.name, &unique_name);
        view.name = unique_name;
    }
    Ok(view)
}

/// Resolve one of the three D-03 worked-example presets (`"twilio"` /
/// `"n8n"` / `"crm"`) as a [`WebhookRouteView`] draft — the wizard's preset
/// tiles call this instead of constructing a `WebhookRoute` directly
/// (module doc: that type does not exist on the wasm client). `scope` lets
/// the draft's name/path avoid a route already configured there (CR-01's
/// unique-default-name half — see [`preset_webhook_route_impl`]).
#[server]
pub async fn preset_webhook_route(
    kind: String,
    scope: ConfigScope,
) -> Result<WebhookRouteView, ServerFnError> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        preset_webhook_route_impl(&kind, &scope).map_err(ServerFnError::new)
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (kind, scope);
        unreachable!("server fn body never runs on the wasm client")
    }
}

/// Parse a drop-in pasted route snippet (D-03) into a [`WebhookRouteView`]
/// draft. See [`parse_route_snippet`] — native-only, exposed here for the
/// wasm client.
#[server]
pub async fn parse_pasted_route(text: String) -> Result<WebhookRouteView, ServerFnError> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let route = parse_route_snippet(&text).map_err(ServerFnError::new)?;
        Ok(WebhookRouteView::from_route(&route))
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = text;
        unreachable!("server fn body never runs on the wasm client")
    }
}

/// Create or replace a route (add or replace by `name`, D-04). Refuses
/// before touching config when a route field or env-NAME field is
/// malformed, the write gate is closed, the path collides with another
/// route, or (CR-01) `name` already exists in `scope` and
/// `allow_overwrite` is `false` — narrowed by `editing_name` (CR-02) to
/// exempt exactly the route being edited in place, so an ordinary
/// unrenamed save is never treated as its own collision.
/// `RouteEditorModal`'s CONFIRM REPLACE button (Task 2) is the ONLY caller
/// that passes `allow_overwrite: true`. `editing_name` is the name the
/// route was opened under — `None` when creating — a client-supplied
/// identity ASSERTION, not proof: see [`upsert_webhook_route_impl`]'s doc
/// for what authority it does and does not grant.
#[server]
pub async fn upsert_webhook_route(
    scope: ConfigScope,
    payload: WebhookRouteView,
    allow_overwrite: bool,
    editing_name: Option<String>,
) -> Result<WebhookRouteView, ServerFnError> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        upsert_webhook_route_impl(scope, payload, allow_overwrite, editing_name)
            .await
            .map_err(|errors| ServerFnError::new(errors.join("; ")))
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (scope, payload, allow_overwrite, editing_name);
        unreachable!("server fn body never runs on the wasm client")
    }
}

/// Delete a route by `name` for `scope`. Idempotent — deleting a
/// non-existent name is not an error.
#[server]
pub async fn delete_webhook_route(scope: ConfigScope, name: String) -> Result<(), ServerFnError> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        delete_webhook_route_impl(scope, name)
            .await
            .map_err(ServerFnError::new)
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (scope, name);
        unreachable!("server fn body never runs on the wasm client")
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;
    use ironhermes_core::config::{Config, SecurityConfig};

    fn seeded_config(write_enabled: bool) -> Config {
        Config {
            security: SecurityConfig {
                web_config_write_enabled: write_enabled,
                ..Default::default()
            },
            ..Config::default()
        }
    }

    // -------------------------------------------------------------------
    // DTO shape — no-secret-value test (env-NAME fields legitimately
    // appear; token/app_token/api_key never do).
    // -------------------------------------------------------------------

    /// Recursively walk a serialized JSON value and assert none of the
    /// FORBIDDEN secret-VALUE key names appears at any nesting depth.
    /// Unlike `platform_config_api`/`gateway_env_secret_api`'s copy of this
    /// helper, this variant explicitly documents that `secret_env`/
    /// `auth_token_env`/`public_key_env`/`outbound_auth_env`/
    /// `outbound_auth_user_env`/`outbound_auth_pass_env` MAY legitimately
    /// appear — they hold a variable NAME, not a value
    /// (49.3-PATTERNS.md's "Env-var-NAME fields ... get their own
    /// no-secret-test variant" note).
    fn assert_no_secret_value_key_at_any_depth(value: &serde_json::Value) {
        match value {
            serde_json::Value::Object(map) => {
                for forbidden in ["token", "app_token", "api_key", "secret", "value"] {
                    assert!(
                        !map.contains_key(forbidden),
                        "serialized DTO must never carry a field named '{forbidden}' at any nesting depth"
                    );
                }
                for v in map.values() {
                    assert_no_secret_value_key_at_any_depth(v);
                }
            }
            serde_json::Value::Array(items) => {
                for v in items {
                    assert_no_secret_value_key_at_any_depth(v);
                }
            }
            _ => {}
        }
    }

    #[test]
    fn webhook_route_view_dto_carries_no_secret_value_field() {
        let route = n8n_generic_preset();
        let view = WebhookRouteView::from_route(&route);
        let json = serde_json::to_value(&view).expect("DTO must serialize");
        assert_no_secret_value_key_at_any_depth(&json);
        // The env-NAME fields legitimately DO appear — this is the
        // documented allow-list, not an oversight.
        assert_eq!(
            json.get("secret_env").and_then(|v| v.as_str()),
            Some("N8N_WEBHOOK_SECRET"),
            "env-NAME fields (holding a variable NAME, not a value) are allowed in the DTO"
        );
        assert_eq!(
            json.get("outbound_auth_env").and_then(|v| v.as_str()),
            Some("N8N_CALLBACK_TOKEN"),
            "the flattened outbound-auth env-NAME field is allowed in the DTO"
        );
    }

    // -------------------------------------------------------------------
    // Preset constructors — doc-faithful to WEBHOOK-AND-REST-API.md.
    // -------------------------------------------------------------------

    #[test]
    fn twilio_preset_is_signature_twilio_and_inbound_only_delivery() {
        use ironhermes_core::webhook_route::{DeliverTarget, SignatureKind};
        let route = twilio_sms_preset();
        assert_eq!(route.signature, SignatureKind::Twilio);
        assert_eq!(route.deliver, DeliverTarget::Platform);
        assert_eq!(route.deliver_platform.as_deref(), Some("telegram"));
        // Inbound-only (D-01/D-02): the preset carries no outbound-SMS
        // affordance — deliver never targets the sender back over SMS.
        assert!(
            !route.deliver_only,
            "twilio preset still runs an agent turn (inbound-only, not a bare echo)"
        );
    }

    #[test]
    fn n8n_preset_matches_worked_example() {
        use ironhermes_core::webhook_route::{DeliverTarget, OutboundAuth, SignatureKind};
        let route = n8n_generic_preset();
        assert_eq!(route.signature, SignatureKind::GenericV2);
        assert_eq!(route.deliver, DeliverTarget::Url);
        assert_eq!(
            route.outbound_auth,
            OutboundAuth::Bearer {
                env: "N8N_CALLBACK_TOKEN".to_string()
            }
        );
    }

    #[test]
    fn crm_preset_is_deliver_only_with_basic_auth() {
        use ironhermes_core::webhook_route::OutboundAuth;
        let route = crm_deliver_only_preset();
        assert!(route.deliver_only, "CRM preset must never run an agent turn");
        assert_eq!(
            route.outbound_auth,
            OutboundAuth::Basic {
                user_env: "TWENTY_CRM_API_USER".to_string(),
                pass_env: "TWENTY_CRM_API_PASSWORD".to_string(),
            }
        );
    }

    // -------------------------------------------------------------------
    // route_would_refuse — truth table (operates on the wasm-safe DTO).
    // -------------------------------------------------------------------

    #[test]
    fn route_would_refuse_truth_table() {
        let none_route = WebhookRouteView::from_route(&ironhermes_core::webhook_route::WebhookRoute {
            signature: ironhermes_core::webhook_route::SignatureKind::None,
            ..Default::default()
        });
        let verified_route =
            WebhookRouteView::from_route(&ironhermes_core::webhook_route::WebhookRoute {
                signature: ironhermes_core::webhook_route::SignatureKind::GenericV2,
                ..Default::default()
            });

        assert!(
            route_would_refuse(&none_route, "0.0.0.0"),
            "signature:none on a non-loopback host must refuse"
        );
        assert!(
            !route_would_refuse(&none_route, "127.0.0.1"),
            "signature:none on loopback must NOT refuse"
        );
        assert!(
            !route_would_refuse(&verified_route, "0.0.0.0"),
            "a verified signature scheme never refuses regardless of bind host"
        );
        assert!(
            route_would_refuse(&none_route, "not-an-ip"),
            "an unparseable bind host is treated conservatively as would-refuse"
        );
    }

    // -------------------------------------------------------------------
    // {}-round-trip — a bare object deserializes with server defaults.
    // -------------------------------------------------------------------

    #[test]
    fn empty_object_round_trips_to_defaulted_route() {
        use ironhermes_core::webhook_route::{SessionMode, SignatureKind};
        let route = parse_route_snippet("{}").expect("bare {} must parse");
        assert_eq!(route.signature, SignatureKind::GenericV2);
        assert_eq!(route.session, SessionMode::Ephemeral);
        assert_eq!(route.rails.max_body_bytes, 1024 * 1024);
    }

    #[test]
    fn yaml_snippet_parses_when_json_fails() {
        let yaml = "name: my-route\npath: /webhook/my-route\nsignature: generic_v2\n";
        let route = parse_route_snippet(yaml).expect("YAML snippet must parse");
        assert_eq!(route.name, "my-route");
        assert_eq!(route.path, "/webhook/my-route");
    }

    // -------------------------------------------------------------------
    // Env-NAME validation.
    // -------------------------------------------------------------------

    #[test]
    fn validate_route_env_names_rejects_lowercase_and_accepts_uppercase() {
        let mut view = WebhookRouteView::from_route(&n8n_generic_preset());
        assert!(
            validate_route_env_names(&view).is_ok(),
            "the preset's own env names must already be valid"
        );

        view.secret_env = Some("lowercase_bad".to_string());
        assert!(
            validate_route_env_names(&view).is_err(),
            "a lowercase env name must be rejected"
        );
    }

    // -------------------------------------------------------------------
    // Bind-host read — the wizard's client-side refusal mirror input.
    // -------------------------------------------------------------------

    #[test]
    fn bind_host_is_none_when_webhook_block_absent() {
        let _g = crate::server::test_support::env_lock();
        let home_dir = tempfile::tempdir().expect("tempdir");
        unsafe { std::env::set_var("IRONHERMES_HOME", home_dir.path()) };

        seeded_config(true).save().expect("seed root config.yaml");
        let result = get_webhook_bind_host_impl(&ConfigScope::Root);
        unsafe { std::env::remove_var("IRONHERMES_HOME") };

        assert_eq!(result.expect("read must succeed"), None);
    }

    #[test]
    fn bind_host_reflects_configured_host() {
        let _g = crate::server::test_support::env_lock();
        let home_dir = tempfile::tempdir().expect("tempdir");
        unsafe { std::env::set_var("IRONHERMES_HOME", home_dir.path()) };

        let mut cfg = seeded_config(true);
        let webhook_platform = ironhermes_core::config::PlatformGatewayConfig {
            host: Some("0.0.0.0".to_string()),
            ..Default::default()
        };
        cfg.gateway
            .platforms
            .insert(WEBHOOK_PLATFORM_KEY.to_string(), webhook_platform);
        cfg.save().expect("seed root config.yaml");

        let result = get_webhook_bind_host_impl(&ConfigScope::Root);
        unsafe { std::env::remove_var("IRONHERMES_HOME") };

        assert_eq!(result.expect("read must succeed"), Some("0.0.0.0".to_string()));
    }

    // -------------------------------------------------------------------
    // CRUD — disk-backed tempdir round trip.
    // -------------------------------------------------------------------

    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn crud_round_trip_upsert_then_delete_leaves_routes_empty_on_disk() {
        let _g = crate::server::test_support::env_lock();
        let home_dir = tempfile::tempdir().expect("tempdir");
        unsafe { std::env::set_var("IRONHERMES_HOME", home_dir.path()) };

        seeded_config(true).save().expect("seed root config.yaml");

        let payload = WebhookRouteView::from_route(&n8n_generic_preset());
        let upsert_result =
            upsert_webhook_route_impl(ConfigScope::Root, payload.clone(), false, None).await;

        let listed = list_webhook_routes_impl(&ConfigScope::Root);
        let delete_result =
            delete_webhook_route_impl(ConfigScope::Root, payload.name.clone()).await;
        let reloaded = ironhermes_core::config::Config::load();
        unsafe { std::env::remove_var("IRONHERMES_HOME") };

        let saved = upsert_result.expect("upsert must succeed when the gate is open");
        assert_eq!(saved.name, payload.name);

        let listed = listed.expect("list must succeed");
        assert_eq!(listed.len(), 1, "exactly one route must be present after upsert");
        assert_eq!(listed[0].name, payload.name);

        delete_result.expect("delete must succeed");

        let reloaded = reloaded.expect("reload saved config");
        let routes_after_delete = reloaded
            .gateway
            .platforms
            .get(WEBHOOK_PLATFORM_KEY)
            .map(|p| p.routes.len())
            .unwrap_or(0);
        assert_eq!(
            routes_after_delete, 0,
            "routes must be empty on disk after delete, re-read confirms it"
        );
    }

    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn upsert_replaces_by_name_not_append() {
        let _g = crate::server::test_support::env_lock();
        let home_dir = tempfile::tempdir().expect("tempdir");
        unsafe { std::env::set_var("IRONHERMES_HOME", home_dir.path()) };

        seeded_config(true).save().expect("seed root config.yaml");

        let mut payload = WebhookRouteView::from_route(&n8n_generic_preset());
        upsert_webhook_route_impl(ConfigScope::Root, payload.clone(), false, None)
            .await
            .expect("first upsert must succeed");

        payload.prompt_template = "updated template".to_string();
        // Same name as the first call — this is now, by design, an
        // EXPLICITLY confirmed replacement (CR-01): `allow_overwrite: true`.
        // No `editing_name` asserted — this models a confirmed overwrite
        // reachable regardless of client identity, orthogonal to CR-02.
        upsert_webhook_route_impl(ConfigScope::Root, payload.clone(), true, None)
            .await
            .expect("second upsert (same name, confirmed) must succeed");

        let listed = list_webhook_routes_impl(&ConfigScope::Root);
        unsafe { std::env::remove_var("IRONHERMES_HOME") };

        let listed = listed.expect("list must succeed");
        assert_eq!(listed.len(), 1, "a confirmed same-name upsert must replace, not append");
        assert_eq!(listed[0].prompt_template, "updated template");
    }

    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn upsert_refuses_when_gate_closed_before_touching_config() {
        let _g = crate::server::test_support::env_lock();
        let home_dir = tempfile::tempdir().expect("tempdir");
        unsafe { std::env::set_var("IRONHERMES_HOME", home_dir.path()) };

        seeded_config(false).save().expect("seed root config.yaml with writes disabled");
        let config_path = ironhermes_core::config::Config::config_path();
        let before = std::fs::read(&config_path).expect("read seeded config");

        let payload = WebhookRouteView::from_route(&n8n_generic_preset());
        let result = upsert_webhook_route_impl(ConfigScope::Root, payload, false, None).await;

        let after = std::fs::read(&config_path).expect("read config after refused write");
        unsafe { std::env::remove_var("IRONHERMES_HOME") };

        assert!(result.is_err(), "upsert must be refused when the gate is closed");
        assert_eq!(before, after, "a refused write must leave disk bytes unchanged");
    }

    // -------------------------------------------------------------------
    // unique_route_name / derive_preset_path — pure de-collision helpers.
    // -------------------------------------------------------------------

    #[test]
    fn unique_route_name_suffixes_until_free() {
        assert_eq!(
            unique_route_name("n8n-trigger", &["n8n-trigger".to_string()]),
            "n8n-trigger-2"
        );
        assert_eq!(
            unique_route_name(
                "n8n-trigger",
                &["n8n-trigger".to_string(), "n8n-trigger-2".to_string()]
            ),
            "n8n-trigger-3"
        );
        assert_eq!(
            unique_route_name("n8n-trigger", &[]),
            "n8n-trigger",
            "no collision must leave the base name unchanged"
        );
    }

    #[test]
    fn derive_preset_path_tracks_the_unique_name_and_leaves_unrelated_paths_alone() {
        assert_eq!(
            derive_preset_path("/webhook/n8n-trigger", "n8n-trigger", "n8n-trigger-2"),
            "/webhook/n8n-trigger-2"
        );
        assert_eq!(
            derive_preset_path("/custom/callback", "n8n-trigger", "n8n-trigger-2"),
            "/custom/callback",
            "a path that does not track the name must be left unchanged"
        );
    }

    // -------------------------------------------------------------------
    // validate_route_fields — WR-01.
    // -------------------------------------------------------------------

    #[test]
    fn validate_route_fields_rejects_missing_leading_slash_and_capture_segments() {
        let mut view = WebhookRouteView::from_route(&n8n_generic_preset());
        view.path = "no-leading-slash".to_string();
        assert!(
            validate_route_fields(&view).is_err(),
            "a path missing the leading slash must be rejected"
        );

        view.path = "/webhook/{id}".to_string();
        assert!(
            validate_route_fields(&view).is_err(),
            "a path with a capture segment must be rejected"
        );
    }

    #[test]
    fn validate_route_fields_accumulates_every_problem_in_one_pass() {
        let mut view = WebhookRouteView::from_route(&n8n_generic_preset());
        view.name = String::new();
        view.path = "missing-slash".to_string();
        let errors = validate_route_fields(&view).expect_err("both name and path are invalid");
        assert!(
            errors.len() >= 2,
            "both problems must be reported in one pass: {errors:?}"
        );
    }

    #[test]
    fn validate_route_fields_errors_never_contain_the_offending_value() {
        let mut view = WebhookRouteView::from_route(&n8n_generic_preset());
        let overlong_name = "x".repeat(MAX_ROUTE_NAME_LEN + 1);
        view.name = overlong_name.clone();
        let errors = validate_route_fields(&view).expect_err("an over-length name must be rejected");
        for e in &errors {
            assert!(
                !e.contains(&overlong_name),
                "error message must never echo the offending value: {e}"
            );
        }
    }

    // -------------------------------------------------------------------
    // validate_route_fields — WR-05 tag-field vocabulary check.
    // -------------------------------------------------------------------

    #[test]
    fn validate_route_fields_rejects_an_out_of_vocabulary_signature() {
        let mut view = WebhookRouteView::from_route(&n8n_generic_preset());
        view.signature = "bogus".to_string();
        let errors =
            validate_route_fields(&view).expect_err("an unrecognized signature must be rejected");
        assert!(
            errors.contains(&"signature must be one of: generic_v2, none, twilio, telnyx".to_string()),
            "errors: {errors:?}"
        );
    }

    #[test]
    fn validate_route_fields_rejects_an_out_of_vocabulary_deliver_target() {
        let mut view = WebhookRouteView::from_route(&n8n_generic_preset());
        view.deliver = "bogus".to_string();
        let errors = validate_route_fields(&view)
            .expect_err("an unrecognized deliver target must be rejected");
        assert!(
            errors.contains(&"deliver must be one of: url, origin, platform".to_string()),
            "errors: {errors:?}"
        );
    }

    #[test]
    fn validate_route_fields_rejects_an_out_of_vocabulary_session_mode() {
        let mut view = WebhookRouteView::from_route(&n8n_generic_preset());
        view.session = "bogus".to_string();
        let errors = validate_route_fields(&view)
            .expect_err("an unrecognized session mode must be rejected");
        assert!(
            errors.contains(&"session must be one of: ephemeral, persistent".to_string()),
            "errors: {errors:?}"
        );
    }

    #[test]
    fn validate_route_fields_rejects_an_out_of_vocabulary_outbound_auth_kind() {
        let mut view = WebhookRouteView::from_route(&n8n_generic_preset());
        view.outbound_auth_kind = "bogus".to_string();
        let errors = validate_route_fields(&view)
            .expect_err("an unrecognized outbound_auth_kind must be rejected — this is WR-05's \
                         most consequential case: an unvalidated value here used to silently \
                         drop a configured outbound Authorization header");
        assert!(
            errors.contains(&"outbound_auth_kind must be one of: none, bearer, basic".to_string()),
            "errors: {errors:?}"
        );
    }

    #[test]
    fn validate_route_fields_accumulates_all_four_tag_field_errors_together() {
        let mut view = WebhookRouteView::from_route(&n8n_generic_preset());
        view.signature = "bogus".to_string();
        view.deliver = "bogus".to_string();
        view.session = "bogus".to_string();
        view.outbound_auth_kind = "bogus".to_string();
        let errors = validate_route_fields(&view).expect_err("all four tag fields are invalid");
        assert_eq!(
            errors.len(),
            4,
            "all four problems must be reported in one pass, matching the module's accumulate \
             discipline: {errors:?}"
        );
    }

    #[test]
    fn validate_route_fields_tag_vocabulary_errors_never_contain_the_offending_value() {
        let mut view = WebhookRouteView::from_route(&n8n_generic_preset());
        view.outbound_auth_kind = "totally-bogus-value".to_string();
        let errors = validate_route_fields(&view).expect_err("an unrecognized value must be rejected");
        for e in &errors {
            assert!(
                !e.contains("totally-bogus-value"),
                "error message must never echo the offending value: {e}"
            );
        }
    }

    #[test]
    fn validate_route_fields_every_value_the_rendered_selects_can_produce_validates_clean() {
        // Built from the existing preset fixtures rather than hand-written
        // tag strings, so this test tracks the real vocabulary if a preset
        // ever changes (matches the module's existing preset-fixture-reuse
        // discipline).
        for preset in [
            twilio_sms_preset(),
            n8n_generic_preset(),
            crm_deliver_only_preset(),
        ] {
            let view = WebhookRouteView::from_route(&preset);
            let result = validate_route_fields(&view);
            assert!(
                result.is_ok(),
                "preset '{}' must validate clean: {result:?}",
                view.name
            );
        }
    }

    // -------------------------------------------------------------------
    // upsert_webhook_route_impl — CR-01/CR-02 collision + duplicate-path +
    // rename guards.
    // -------------------------------------------------------------------

    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn upsert_refuses_unconfirmed_name_collision_and_leaves_disk_bytes_unchanged() {
        let _g = crate::server::test_support::env_lock();
        let home_dir = tempfile::tempdir().expect("tempdir");
        unsafe { std::env::set_var("IRONHERMES_HOME", home_dir.path()) };

        seeded_config(true).save().expect("seed root config.yaml");
        let payload = WebhookRouteView::from_route(&n8n_generic_preset());
        upsert_webhook_route_impl(ConfigScope::Root, payload.clone(), false, None)
            .await
            .expect("first upsert must succeed (no collision yet)");

        let config_path = ironhermes_core::config::Config::config_path();
        let before = std::fs::read(&config_path).expect("read config after first upsert");

        // CR-02: models a GENUINELY DIFFERENT client — a stale route list, a
        // second tab, a hand-crafted call — asserting NO editing identity,
        // not the route being edited resubmitting its own unchanged name
        // (that case is `upsert_accepts_an_in_place_edit_under_its_own_name_
        // without_confirmation` below, and must succeed).
        let mut colliding = payload.clone();
        colliding.prompt_template = "attempted unconfirmed overwrite".to_string();
        let result = upsert_webhook_route_impl(ConfigScope::Root, colliding, false, None).await;

        let after = std::fs::read(&config_path).expect("read config after refused collision");
        unsafe { std::env::remove_var("IRONHERMES_HOME") };

        assert!(
            result.is_err(),
            "an unconfirmed name collision from a client asserting no editing identity must be refused"
        );
        assert_eq!(
            before, after,
            "a refused collision must leave disk bytes byte-identical"
        );
    }

    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn upsert_accepts_an_in_place_edit_under_its_own_name_without_confirmation() {
        let _g = crate::server::test_support::env_lock();
        let home_dir = tempfile::tempdir().expect("tempdir");
        unsafe { std::env::set_var("IRONHERMES_HOME", home_dir.path()) };

        seeded_config(true).save().expect("seed root config.yaml");
        let mut payload = WebhookRouteView::from_route(&n8n_generic_preset());
        upsert_webhook_route_impl(ConfigScope::Root, payload.clone(), false, None)
            .await
            .expect("first upsert (create) must succeed");

        payload.prompt_template = "in-place edit".to_string();
        let name = payload.name.clone();
        // CR-02: the SAME name, `editing_name` asserting that identity,
        // `allow_overwrite: false` — this is the ordinary in-place edit and
        // must succeed with no confirmation (49.3-07-PLAN.md's must-have
        // truth for the unrenamed edit, restored).
        let result = upsert_webhook_route_impl(
            ConfigScope::Root,
            payload.clone(),
            false,
            Some(name.clone()),
        )
        .await;
        let listed = list_webhook_routes_impl(&ConfigScope::Root);
        unsafe { std::env::remove_var("IRONHERMES_HOME") };

        result.expect("an in-place edit asserting its own identity must succeed unconfirmed");
        let listed = listed.expect("list must succeed");
        assert_eq!(listed.len(), 1, "an in-place edit must not add a second route");
        assert_eq!(listed[0].prompt_template, "in-place edit");
    }

    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn upsert_refuses_a_rename_onto_another_routes_name_without_confirmation() {
        let _g = crate::server::test_support::env_lock();
        let home_dir = tempfile::tempdir().expect("tempdir");
        unsafe { std::env::set_var("IRONHERMES_HOME", home_dir.path()) };

        seeded_config(true).save().expect("seed root config.yaml");
        let first = WebhookRouteView::from_route(&n8n_generic_preset());
        upsert_webhook_route_impl(ConfigScope::Root, first.clone(), false, None)
            .await
            .expect("first upsert must succeed");
        let second = WebhookRouteView::from_route(&crm_deliver_only_preset());
        upsert_webhook_route_impl(ConfigScope::Root, second.clone(), false, None)
            .await
            .expect("second upsert must succeed");

        let config_path = ironhermes_core::config::Config::config_path();
        let before = std::fs::read(&config_path).expect("read config after two seeded routes");

        // Editing `second`, renaming it onto `first`'s name — `editing_name`
        // correctly names the route being edited (`second`'s own name), but
        // the TARGET name collides with a DIFFERENT route (`first`). CR-01's
        // protection must still fire: the narrowing is exactly the route
        // being edited, not every collision.
        let mut renamed = second.clone();
        renamed.name = first.name.clone();
        let result = upsert_webhook_route_impl(
            ConfigScope::Root,
            renamed,
            false,
            Some(second.name.clone()),
        )
        .await;

        let after = std::fs::read(&config_path).expect("read config after refused rename");
        unsafe { std::env::remove_var("IRONHERMES_HOME") };

        assert!(
            result.is_err(),
            "a rename onto another existing route's name must still be refused unconfirmed"
        );
        assert_eq!(before, after, "a refused rename must leave disk bytes unchanged");
    }

    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn upsert_renaming_to_a_free_name_moves_the_route_instead_of_orphaning_it() {
        let _g = crate::server::test_support::env_lock();
        let home_dir = tempfile::tempdir().expect("tempdir");
        unsafe { std::env::set_var("IRONHERMES_HOME", home_dir.path()) };

        seeded_config(true).save().expect("seed root config.yaml");
        // Seed a SECOND, unrelated route too — proves the duplicate-path
        // guard's `editing_name` exemption is scoped to the route being
        // renamed, not a blanket bypass (`upsert_refuses_a_path_already_
        // used_by_another_route` below covers the still-refused direction
        // unchanged).
        let mut payload = WebhookRouteView::from_route(&n8n_generic_preset());
        let old_name = payload.name.clone();
        let unchanged_path = payload.path.clone();
        upsert_webhook_route_impl(ConfigScope::Root, payload.clone(), false, None)
            .await
            .expect("first upsert (create) must succeed");
        let other = WebhookRouteView::from_route(&crm_deliver_only_preset());
        upsert_webhook_route_impl(ConfigScope::Root, other.clone(), false, None)
            .await
            .expect("seeding the unrelated second route must succeed");

        // Rename the FIRST route to a free name while KEEPING its own path
        // — must not be refused by the duplicate-path guard (CR-02's
        // `editing_name` exemption), and must MOVE the route rather than
        // appending a second entry under the new name (D-04).
        payload.name = "n8n-trigger-renamed".to_string();
        let result = upsert_webhook_route_impl(
            ConfigScope::Root,
            payload.clone(),
            false,
            Some(old_name.clone()),
        )
        .await;
        let listed = list_webhook_routes_impl(&ConfigScope::Root);
        unsafe { std::env::remove_var("IRONHERMES_HOME") };

        result.expect(
            "renaming to a free name while keeping the route's own path must succeed \
             (the duplicate-path guard must not refuse a route against itself)",
        );
        let listed = listed.expect("list must succeed");
        assert_eq!(
            listed.len(),
            2,
            "a rename must MOVE the route, not add a second entry (D-04) — total stays at 2"
        );
        assert!(
            listed.iter().all(|r| r.name != old_name),
            "the old name must be gone from config.yaml after the rename"
        );
        let renamed = listed
            .iter()
            .find(|r| r.name == "n8n-trigger-renamed")
            .expect("the renamed route must be present under its new name");
        assert_eq!(
            renamed.path, unchanged_path,
            "the path must be unchanged across the rename"
        );
    }

    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn upsert_replaces_existing_route_when_overwrite_is_confirmed() {
        let _g = crate::server::test_support::env_lock();
        let home_dir = tempfile::tempdir().expect("tempdir");
        unsafe { std::env::set_var("IRONHERMES_HOME", home_dir.path()) };

        seeded_config(true).save().expect("seed root config.yaml");
        let mut payload = WebhookRouteView::from_route(&n8n_generic_preset());
        upsert_webhook_route_impl(ConfigScope::Root, payload.clone(), false, None)
            .await
            .expect("first upsert must succeed");

        payload.prompt_template = "confirmed replacement".to_string();
        let result =
            upsert_webhook_route_impl(ConfigScope::Root, payload.clone(), true, None).await;
        let listed = list_webhook_routes_impl(&ConfigScope::Root);
        unsafe { std::env::remove_var("IRONHERMES_HOME") };

        result.expect("a confirmed overwrite must succeed (the pre-CR-01 behavior, now reachable only by explicit confirmation)");
        let listed = listed.expect("list must succeed");
        assert_eq!(listed.len(), 1, "a confirmed replace must replace, not append");
        assert_eq!(listed[0].prompt_template, "confirmed replacement");
    }

    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn upsert_refuses_a_path_already_used_by_another_route() {
        let _g = crate::server::test_support::env_lock();
        let home_dir = tempfile::tempdir().expect("tempdir");
        unsafe { std::env::set_var("IRONHERMES_HOME", home_dir.path()) };

        seeded_config(true).save().expect("seed root config.yaml");
        let first = WebhookRouteView::from_route(&n8n_generic_preset());
        upsert_webhook_route_impl(ConfigScope::Root, first.clone(), false, None)
            .await
            .expect("first upsert must succeed");

        let mut second = WebhookRouteView::from_route(&crm_deliver_only_preset());
        second.path = first.path.clone();
        let result = upsert_webhook_route_impl(ConfigScope::Root, second, false, None).await;
        unsafe { std::env::remove_var("IRONHERMES_HOME") };

        assert!(
            result.is_err(),
            "a duplicate path across two different-named routes must be refused"
        );
    }

    // -------------------------------------------------------------------
    // preset_webhook_route_impl — de-collided preset drafts.
    // -------------------------------------------------------------------

    #[test]
    fn preset_route_name_and_path_avoid_an_existing_route_in_scope() {
        let _g = crate::server::test_support::env_lock();
        let home_dir = tempfile::tempdir().expect("tempdir");
        unsafe { std::env::set_var("IRONHERMES_HOME", home_dir.path()) };

        // Seed a config that already holds the n8n preset under its default
        // name/path.
        let mut cfg = seeded_config(true);
        let mut platform = ironhermes_core::config::PlatformGatewayConfig::default();
        platform.routes.push(n8n_generic_preset());
        cfg.gateway
            .platforms
            .insert(WEBHOOK_PLATFORM_KEY.to_string(), platform);
        cfg.save().expect("seed config with an existing n8n route");

        let draft = preset_webhook_route_impl("n8n", &ConfigScope::Root)
            .expect("resolving the preset again must still succeed");
        unsafe { std::env::remove_var("IRONHERMES_HOME") };

        let existing = n8n_generic_preset();
        assert_ne!(
            draft.name, existing.name,
            "resolving the same preset a second time must not collide by default"
        );
        assert_ne!(
            draft.path, existing.path,
            "the de-collided draft's path must also differ"
        );
    }

    // -------------------------------------------------------------------
    // Combined client+server contract (CR-02) — the test class whose
    // absence let CR-02 ship past two individually-green suites. Each test
    // feeds the CLIENT predicate's own output into the SERVER impl
    // unmodified — no value is retyped between the two halves, so if a
    // future change makes them disagree again, these tests fail.
    // -------------------------------------------------------------------

    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn client_save_intent_for_an_in_place_edit_produces_arguments_the_server_accepts() {
        use crate::components::hermes_app::screens::gateway::webhook_wizard::{
            save_intent, SaveIntent,
        };

        let _g = crate::server::test_support::env_lock();
        let home_dir = tempfile::tempdir().expect("tempdir");
        unsafe { std::env::set_var("IRONHERMES_HOME", home_dir.path()) };

        seeded_config(true).save().expect("seed root config.yaml");
        let mut payload = WebhookRouteView::from_route(&n8n_generic_preset());
        upsert_webhook_route_impl(ConfigScope::Root, payload.clone(), false, None)
            .await
            .expect("seed the route being edited");

        // The SAME source `RouteEditorModal` reads — `list_webhook_routes_
        // impl`, not a hand-written literal.
        let existing_names: Vec<String> = list_webhook_routes_impl(&ConfigScope::Root)
            .expect("list must succeed")
            .into_iter()
            .map(|r| r.name)
            .collect();

        let initial_name = payload.name.clone();
        payload.prompt_template = "edited via the client predicate".to_string();

        // is_new: false, draft name unchanged — the ordinary in-place edit.
        let intent = save_intent(false, &initial_name, &payload.name, &existing_names);
        let (allow_overwrite, editing_name) = match intent {
            SaveIntent::DirectSend {
                allow_overwrite,
                editing_name,
            } => (allow_overwrite, editing_name),
            SaveIntent::Confirm { .. } => panic!(
                "CR-02: an in-place edit must ask for a direct send, never the REPLACE ROUTE confirm"
            ),
        };

        // Feed the client's OWN direct-send output into the server impl,
        // UNMODIFIED — this is the boundary CR-02 broke.
        let result = upsert_webhook_route_impl(
            ConfigScope::Root,
            payload.clone(),
            allow_overwrite,
            editing_name,
        )
        .await;
        let listed = list_webhook_routes_impl(&ConfigScope::Root);
        unsafe { std::env::remove_var("IRONHERMES_HOME") };

        result.expect(
            "CR-02: the client predicate's own direct-send output must be accepted by the \
             server impl for an in-place edit",
        );
        let listed = listed.expect("list must succeed");
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].prompt_template, "edited via the client predicate");
    }

    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn client_save_intent_and_server_guard_agree_on_a_genuine_collision() {
        use crate::components::hermes_app::screens::gateway::webhook_wizard::{
            save_intent, SaveIntent,
        };

        let _g = crate::server::test_support::env_lock();
        let home_dir = tempfile::tempdir().expect("tempdir");
        unsafe { std::env::set_var("IRONHERMES_HOME", home_dir.path()) };

        seeded_config(true).save().expect("seed root config.yaml");
        let existing = WebhookRouteView::from_route(&n8n_generic_preset());
        upsert_webhook_route_impl(ConfigScope::Root, existing.clone(), false, None)
            .await
            .expect("seed the colliding route");

        let existing_names: Vec<String> = list_webhook_routes_impl(&ConfigScope::Root)
            .expect("list must succeed")
            .into_iter()
            .map(|r| r.name)
            .collect();

        // A NEW route (is_new: true) whose draft name collides with the
        // seeded route.
        let mut new_route = WebhookRouteView::from_route(&crm_deliver_only_preset());
        new_route.name = existing.name.clone();

        let intent = save_intent(true, "", &new_route.name, &existing_names);
        let colliding_name = match intent {
            SaveIntent::Confirm { colliding_name } => colliding_name,
            SaveIntent::DirectSend { .. } => panic!(
                "CR-02: a genuine collision must ask for the REPLACE ROUTE confirm, not a direct send"
            ),
        };
        assert_eq!(
            colliding_name, existing.name,
            "the confirm must name the colliding route"
        );

        // The arguments a client would send WITHOUT confirming — the two
        // halves must agree in the refusal direction too.
        let unconfirmed_result =
            upsert_webhook_route_impl(ConfigScope::Root, new_route.clone(), false, None).await;
        unsafe { std::env::remove_var("IRONHERMES_HOME") };

        assert!(
            unconfirmed_result.is_err(),
            "CR-02: the server must still refuse the unconfirmed collision the client \
             predicate flagged for a confirm"
        );
    }
}
