//! D-11 SSRF-gated URL delivery, plus (D-13) named-platform delivery
//! through the shared [`ironhermes_cron::DeliveryRegistry`] handle.
//!
//! Every `deliver: url` target is validated through
//! [`ironhermes_core::ssrf::is_safe_url`] TWICE: once at route-config load
//! time (`validate_route_target`, called from
//! `WebhookAdapter::new`) and again immediately before every delivery POST
//! (`deliver_to_url`). `is_safe_url`'s own module doc names DNS rebinding
//! as a known time-of-check-to-time-of-use limitation — checking twice is
//! the documented mitigation. `is_safe_url` resolves DNS synchronously, so
//! every call from this async module is wrapped in
//! `tokio::task::spawn_blocking`.
//!
//! Both checks validate only the URL handed to them, so the delivery client
//! additionally refuses to follow redirects (see [`build_client`]) — without
//! that, a checked public target could redirect the request to an internal
//! address and neither check would ever see the final hop.

use std::sync::Arc;
use std::time::Duration;

use ironhermes_cron::DeliveryRegistry;

/// Outcome of a single delivery attempt. Refusal (SSRF, non-2xx, transport
/// error) is always a logged, failed delivery — never a silent fallback to
/// an unchecked request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeliveryOutcome {
    Delivered,
    Failed(String),
}

/// Build the one `reqwest::Client` a `WebhookAdapter` uses for every
/// `deliver: url` target — explicit connect + request timeouts, mirroring
/// the construction shape in `ironhermes_hooks::webhook::WebhookDelivery`
/// (the OUTBOUND hook-event delivery client; this module mirrors its
/// SHAPE only — see this crate's module docs on the naming-collision risk,
/// D-02 forbids editing that module).
///
/// Redirect-following is DISABLED (D-11). Both `is_safe_url` checks
/// validate only the URL they are given, so under reqwest's default policy
/// a target on a public IP could pass both checks and then answer
/// `302 Location: http://169.254.169.254/...` — reqwest would follow it and
/// carry the POST body plus the route's env-indirected outbound auth header
/// to an internal address, defeating D-11 entirely.
///
/// `Policy::none()` rather than a `Policy::custom` that re-checks each hop:
/// `is_safe_url` resolves DNS synchronously (which is why every call in this
/// module is wrapped in `spawn_blocking`), and a custom redirect policy runs
/// inline on the connection task, where that blocking resolve would stall the
/// runtime. A `deliver: url` target is an exact operator-configured endpoint,
/// so following redirects buys nothing to offset the risk.
pub fn build_client() -> reqwest::Client {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(10))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap_or_default()
}

/// Route-config-load-time SSRF check (D-11, first of the two checks).
/// Called once per configured `deliver: url` target from
/// `WebhookAdapter::new`, before the listener ever opens — an obviously-bad
/// target is refused before the adapter can even construct.
///
/// **Deliberately synchronous, and therefore DNS-blocking.** This is
/// fail-fast construction-time validation: it belongs in a sync constructor
/// that returns `Result`, not behind an `async fn` that every test and
/// construction site would have to await. The blocking is instead confined at
/// the single async call site — `GatewayRunner::start()` wraps the whole
/// `WebhookAdapter::new` call in `spawn_blocking` (T-36.7.1-09). A new async
/// caller of this function, or of `WebhookAdapter::new`, must do the same.
pub fn validate_route_target(url: &str) -> anyhow::Result<()> {
    if !ironhermes_core::ssrf::is_safe_url(url) {
        anyhow::bail!("delivery target failed SSRF validation at route-config load time: {url}");
    }
    Ok(())
}

/// Delivery-time SSRF check (D-11, second of the two checks) plus the
/// actual POST. `content` is wrapped as `{"content": "..."}` JSON so the
/// receiving endpoint can distinguish this delivery's payload from an
/// empty body. `auth_header` is `(header_name, header_value)`, resolved by
/// the caller from the route's env-indirected `OutboundAuth` at delivery
/// time — this function never logs it.
pub async fn deliver_to_url(
    client: &reqwest::Client,
    url: &str,
    content: &str,
    auth_header: Option<(String, String)>,
) -> DeliveryOutcome {
    let url_owned = url.to_string();
    let safe = tokio::task::spawn_blocking(move || ironhermes_core::ssrf::is_safe_url(&url_owned))
        .await
        .unwrap_or(false);
    if !safe {
        tracing::error!(
            url = %url,
            "webhook deliver_to_url: SSRF check refused delivery target at delivery time (D-11)"
        );
        return DeliveryOutcome::Failed("SSRF check refused delivery target".to_string());
    }

    let mut req = client
        .post(url)
        .json(&serde_json::json!({ "content": content }));
    if let Some((name, value)) = auth_header {
        req = req.header(name, value);
    }

    match req.send().await {
        Ok(resp) if resp.status().is_success() => DeliveryOutcome::Delivered,
        Ok(resp) => {
            let status = resp.status();
            tracing::warn!(url = %url, %status, "webhook deliver_to_url: non-2xx response");
            DeliveryOutcome::Failed(format!("non-2xx status: {status}"))
        }
        Err(e) => {
            tracing::warn!(url = %url, error = %e, "webhook deliver_to_url: request error");
            DeliveryOutcome::Failed(format!("request error: {e}"))
        }
    }
}

/// D-13: deliver via a named platform adapter already registered in the
/// shared [`DeliveryRegistry`] handle (populated by `GatewayRunner::start()`
/// after every platform adapter is constructed — see that function's own
/// doc comment on the ordering problem this shared handle solves).
pub async fn deliver_to_platform(
    registry: &DeliveryRegistry,
    platform: &str,
    chat_id: &str,
    content: &str,
) -> DeliveryOutcome {
    match registry.get(platform) {
        Some(sender) => match sender.send_text(chat_id, content, None).await {
            Ok(()) => DeliveryOutcome::Delivered,
            Err(e) => {
                tracing::warn!(
                    platform = %platform,
                    chat_id = %chat_id,
                    error = %e,
                    "webhook deliver_to_platform: send_text failed"
                );
                DeliveryOutcome::Failed(format!("platform delivery error: {e}"))
            }
        },
        None => {
            tracing::warn!(
                platform = %platform,
                "webhook deliver_to_platform: no delivery sender registered for this platform"
            );
            DeliveryOutcome::Failed(format!(
                "no delivery sender registered for platform '{platform}'"
            ))
        }
    }
}

/// Convenience alias used by call sites that already hold the shared,
/// lock-guarded registry handle `GatewayRunner::start()` constructs.
pub type SharedDeliveryRegistry = Arc<tokio::sync::RwLock<DeliveryRegistry>>;
