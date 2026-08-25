//! The INBOUND webhook adapter (`WebhookAdapter` + `run_webhook_adapter`).
//!
//! Not to be confused with `ironhermes_hooks::webhook`, the OUTBOUND
//! hook-event delivery module (HMAC-signing, exponential backoff,
//! disk-persisted retry queue) — that module is untouched by this crate
//! (D-02). This module's direction is opposite: an inbound HTTP POST
//! becomes an agent turn.
//!
//! Follows the `BuzzAdapter` split (`crates/ironhermes-gateway/src/buzz.rs`)
//! exactly: [`WebhookAdapter`] owns configuration/client/verifiers/running
//! flag; the free function [`run_webhook_adapter`] owns the inbound
//! listener and dispatches each accepted request on its own spawned task
//! (D-12 — 202 first, agent turn runs in the background).

pub mod approval;
pub mod deliver;
pub mod idempotency;
pub mod rate_limit;
pub mod route_config;
pub mod template;
pub mod verifier;

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Context, Result, anyhow};
use async_trait::async_trait;
use axum::RequestExt;
use axum::extract::Request;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use ironhermes_core::{MessageEvent, MessageResponse, Platform};
use ironhermes_cron::DeliveryRegistry;
use ironhermes_core::adapter::{MessageHandler, PlatformAdapter};
use tokio::net::TcpListener;
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;

use idempotency::{Clock, IdempotencyCache, SystemClock};
use rate_limit::FixedWindowLimiter;
use route_config::{
    DeliverTarget, OutboundAuth, SessionMode, SignatureKind, WebhookRoute, WebhookRoutesConfig,
};
use verifier::{VerifyRequest, Verifier};

/// The inbound webhook `PlatformAdapter`. Owns the route table, the
/// per-route verifier map, the shared reqwest client, a handle into the
/// shared [`DeliveryRegistry`] (populated by `GatewayRunner::start()` after
/// every platform adapter is constructed — Plan 05 adds platform delivery
/// targets without touching that construction site again), one
/// [`IdempotencyCache`] and one [`FixedWindowLimiter`] per route (D-15,
/// keyed off that route's own `rails.idempotency_ttl_secs` /
/// `rails.rate_limit_per_minute`), and the running flag.
pub struct WebhookAdapter {
    config: WebhookRoutesConfig,
    client: reqwest::Client,
    verifiers: HashMap<String, Verifier>,
    delivery_registry: Arc<RwLock<DeliveryRegistry>>,
    idempotency_caches: HashMap<String, Arc<IdempotencyCache>>,
    rate_limiters: HashMap<String, Arc<FixedWindowLimiter>>,
    /// D-11 origin family: `handle_webhook_post` extracts the caller-supplied
    /// callback URL from a `deliver: origin` request's payload and stashes it
    /// here keyed by that delivery's message id (threaded through as the
    /// `MessageEvent`'s `thread_id`), because `send_message` — called later,
    /// by the external `MessageHandler`, once the agent turn completes — only
    /// receives `(chat_id, content, thread_id)` and `chat_id` is the route
    /// name, shared by every concurrent delivery on that route. Consumed
    /// (removed) by `send_message` on first read.
    ///
    /// **Bounded by TTL and by hard cap** (security audit N-02). This field's
    /// original comment claimed the unclaimed-entry case was "a bounded,
    /// documented leak, not a growth path". That was wrong: one entry is
    /// inserted per REQUEST on a non-`deliver_only` `deliver: origin` route,
    /// and the only removal is in `send_message` — which, per WINDOWS ledger
    /// 17, is not reached for these turns at all today. So it grew with
    /// lifetime request count, holding caller-supplied URLs bounded only by
    /// `rails.max_body_bytes`.
    ///
    /// Now pruned lazily on insert against [`ORIGIN_CALLBACK_TTL`] — the same
    /// prune-on-access discipline [`IdempotencyCache`] uses, and for the same
    /// reason: a background sweeper is another thing to schedule, supervise
    /// and shut down for a map whose own access pattern already provides the
    /// sweep opportunity. [`ORIGIN_CALLBACK_MAX_ENTRIES`] is the backstop for
    /// the case the TTL cannot cover — sustained arrival faster than the TTL
    /// expires entries.
    ///
    /// **Bounded by value size too** (code review WR-03). The TTL and the
    /// entry cap bound how many URLs are retained and for how long, not how
    /// large each one is; the only ceiling on the value was
    /// `rails.max_body_bytes` (1 MiB default), so the cap's worst case was
    /// 10_000 × 1 MiB ≈ 10 GB of hour-pinned memory from a sender that passes
    /// signature verification. [`extract_origin_callback_url`] now refuses any
    /// value longer than [`MAX_CALLBACK_URL_BYTES`], so a body-sized string
    /// never enters this map at all.
    origin_callbacks: std::sync::Mutex<HashMap<String, (String, std::time::Instant)>>,
    /// Time source for [`Self::origin_callbacks`] pruning. Shared with the
    /// per-route caches so a test advancing a `FakeClock` advances all of
    /// them together.
    clock: Arc<dyn Clock>,
    /// `true` when any configured route selects `signature: none` — the
    /// D-10 loopback rail this construction-time and pre-bind check both
    /// key off of.
    requires_loopback: bool,
    running: Arc<AtomicBool>,
}

impl WebhookAdapter {
    /// Construction-time validation (D-06/D-10/D-11/D-17), returning `Err`
    /// rather than panicking — a webhook platform that fails to construct
    /// must not take down the whole gateway process
    /// (`GatewayRunner::start()` logs and skips it, fail-closed).
    ///
    /// Does no I/O — no socket is opened here. That happens later, inside
    /// [`run_webhook_adapter`].
    ///
    /// Validates, per route:
    /// - resolves the route's key material from its named environment
    ///   variable, failing with a message naming the variable (never its
    ///   value) when absent;
    /// - runs every `deliver: url` target through the SSRF check
    ///   (`deliver::validate_route_target`) — D-11's load-time half of the
    ///   double check;
    /// - tracks whether any route selects `signature: none`.
    ///
    /// Then, once, enforces D-10: if any route is `signature: none` and the
    /// configured bind host is not loopback, refuses construction outright
    /// — naming the offending route — so the gateway never boots an
    /// unauthenticated agent endpoint reachable from the network. There is
    /// no operator override.
    pub fn new(
        config: WebhookRoutesConfig,
        delivery_registry: Arc<RwLock<DeliveryRegistry>>,
    ) -> Result<Self> {
        Self::new_with_clock(config, delivery_registry, Arc::new(SystemClock))
    }

    /// Same construction-time validation as [`WebhookAdapter::new`], but
    /// accepting an injectable [`Clock`] threaded into every route's
    /// [`IdempotencyCache`] (and Task 3's rate limiter). Additive — this
    /// does not change `new`'s existing signature or behavior, so every
    /// existing caller (`GatewayRunner::start()`) is unaffected. Tests use
    /// this constructor with a [`idempotency::FakeClock`] to advance
    /// TTL/window-dependent rail state deterministically instead of
    /// sleeping in real time (source_facts #7).
    pub fn new_with_clock(
        config: WebhookRoutesConfig,
        delivery_registry: Arc<RwLock<DeliveryRegistry>>,
        clock: Arc<dyn Clock>,
    ) -> Result<Self> {
        let host_ip: IpAddr = config
            .host
            .parse()
            .with_context(|| format!("webhook adapter: invalid bind host '{}'", config.host))?;

        let mut verifiers = HashMap::with_capacity(config.routes.len());
        let mut idempotency_caches = HashMap::with_capacity(config.routes.len());
        let mut rate_limiters = HashMap::with_capacity(config.routes.len());
        let mut requires_loopback = false;

        // T-36.7.1-01 / security audit N-03: reject duplicate `name` and
        // duplicate `path` here, before anything is inserted or bound.
        //
        // Duplicate NAME silently cross-wires two routes. The three maps below
        // are keyed on `route.name`, so a second route with the same name wins
        // them (last write), while `deliver_for` resolves the delivery target
        // by scanning `config.routes` with `.find()` and gets the FIRST match.
        // A request would then be verified with route B's secret and its answer
        // delivered to route A's target — defeating the per-route secret
        // isolation T-36.7.1-01 exists to provide, with no error anywhere.
        //
        // Duplicate PATH is worse-behaved than it looks: `axum::Router::route`
        // panics on a repeated path, and that panic happens inside the spawned
        // serve task — AFTER `running.store(true)`, so the adapter reports
        // itself started and then dies. Refusing here keeps the failure inside
        // the fail-closed-at-construction contract this type documents.
        let mut seen_names: std::collections::HashSet<&str> = std::collections::HashSet::new();
        let mut seen_paths: std::collections::HashSet<&str> = std::collections::HashSet::new();
        for route in &config.routes {
            if !seen_names.insert(route.name.as_str()) {
                return Err(anyhow!(
                    "webhook adapter refuses to construct: duplicate route name '{}'. \
                     Route names key this adapter's verifier, idempotency and rate-limit \
                     maps, so a duplicate would verify one route's requests with another \
                     route's secret and deliver the answer to the wrong target.",
                    route.name
                ));
            }
            // WR-02: uniqueness is not enough. `axum::Router::route` panics on
            // a strictly larger class than "byte-identical path", verified
            // against the pinned axum-0.8.4 source:
            //   - `path_router.rs:41` — empty path;
            //   - `:43` — "Paths must start with a `/`", so a config typo
            //     `path: "hooks/sms"` panics;
            //   - `:159` — `set_node` propagates a `matchit` insertion
            //     conflict, wrapped by `panic_on_err!`, so `/hooks/{a}` and
            //     `/hooks/{b}` — distinct strings, conflicting routes — panic;
            //   - `:53` — a segment starting with `:` (the v0.7 capture syntax
            //     an operator migrating a config would naturally write).
            // `route.path` is validated nowhere else: `webhook_route.rs` has no
            // path constraint at all. The panic unwinds out of
            // `serve_webhook_adapter` AFTER `running.store(true)` and BEFORE
            // `running.store(false)` (which unwinding skips), so the adapter
            // reports `is_running() == true` for the process lifetime with no
            // listener behind it — outside the fail-closed-at-construction
            // contract this type documents.
            //
            // Rejecting capture segments outright also removes the `matchit`
            // conflict class entirely: two LITERAL paths conflict only when
            // they are equal, which the uniqueness check above already covers.
            if !route.path.starts_with('/') {
                return Err(anyhow!(
                    "webhook adapter refuses to construct: route '{}' has path '{}', which \
                     must start with '/'. axum::Router::route panics on it, inside the \
                     spawned serve task, after the adapter has already reported itself \
                     running.",
                    route.name,
                    route.path
                ));
            }
            if route
                .path
                .split('/')
                .any(|seg| seg.starts_with(':') || seg.starts_with('{'))
            {
                return Err(anyhow!(
                    "webhook adapter refuses to construct: route '{}' has path '{}', which \
                     contains a capture segment. Webhook route paths must be literal — a \
                     capture segment can conflict with another route's path inside axum's \
                     router, which panics in the spawned serve task after the adapter has \
                     already reported itself running.",
                    route.name,
                    route.path
                ));
            }
            if !seen_paths.insert(route.path.as_str()) {
                return Err(anyhow!(
                    "webhook adapter refuses to construct: duplicate route path '{}' \
                     (route '{}'). Two routes cannot share one URL path — the router \
                     would panic after the listener reported itself started.",
                    route.path,
                    route.name
                ));
            }
        }

        for route in &config.routes {
            let verifier = resolve_verifier(route)?;
            if matches!(route.signature, SignatureKind::None) {
                requires_loopback = true;
            }
            verifiers.insert(route.name.clone(), verifier);
            idempotency_caches.insert(
                route.name.clone(),
                Arc::new(IdempotencyCache::with_clock(
                    route.rails.idempotency_ttl_secs,
                    clock.clone(),
                )),
            );
            rate_limiters.insert(
                route.name.clone(),
                Arc::new(FixedWindowLimiter::with_clock(
                    route.rails.rate_limit_per_minute,
                    clock.clone(),
                )),
            );

            if matches!(route.deliver, DeliverTarget::Url) {
                match &route.deliver_url {
                    Some(url) => deliver::validate_route_target(url)
                        .with_context(|| format!("webhook route '{}'", route.name))?,
                    None => {
                        return Err(anyhow!(
                            "webhook route '{}': deliver=url requires deliver_url",
                            route.name
                        ));
                    }
                }
            }
        }

        if requires_loopback && !host_ip.is_loopback() {
            return Err(anyhow!(
                "webhook adapter refuses to construct: at least one route selects \
                 signature=none while the configured bind host '{}' is not loopback — this \
                 would serve an unauthenticated agent endpoint to the network (D-10). There is \
                 no override; either remove the no-verification route or bind to a loopback \
                 address.",
                config.host
            ));
        }

        Ok(Self {
            client: deliver::build_client(),
            verifiers,
            delivery_registry,
            idempotency_caches,
            rate_limiters,
            origin_callbacks: std::sync::Mutex::new(HashMap::new()),
            clock,
            requires_loopback,
            config,
            running: Arc::new(AtomicBool::new(false)),
        })
    }

    /// Record a `deliver: origin` request's caller-supplied callback URL under
    /// this delivery's id, pruning expired entries as a side effect of the
    /// same call (security audit N-02).
    ///
    /// Returns `false` when the hard cap is reached and nothing could be
    /// expired to make room — the delivery proceeds, but its answer will fail
    /// loudly at `send_message` rather than being silently mis-delivered.
    fn record_origin_callback(&self, delivery_id: String, url: String) -> bool {
        let now = self.clock.now();
        let mut map = self.origin_callbacks.lock().unwrap();
        map.retain(|_, (_, inserted_at)| {
            now.duration_since(*inserted_at) < ORIGIN_CALLBACK_TTL
        });
        if map.len() >= ORIGIN_CALLBACK_MAX_ENTRIES && !map.contains_key(&delivery_id) {
            tracing::error!(
                delivery_id = %delivery_id,
                live_entries = map.len(),
                "webhook deliver=origin: callback table is at its {ORIGIN_CALLBACK_MAX_ENTRIES}-entry \
                 cap with nothing expired to evict — refusing to record this delivery's callback \
                 URL. Its answer will fail loudly rather than be delivered to another caller."
            );
            return false;
        }
        map.insert(delivery_id, (url, now));
        true
    }

    /// Live entry count for [`Self::origin_callbacks`] — used by tests to
    /// assert the table bounds itself without an external sweeper.
    #[doc(hidden)]
    pub fn origin_callback_entry_count(&self) -> usize {
        self.origin_callbacks.lock().unwrap().len()
    }

    /// Test-only door onto [`Self::record_origin_callback`], which is
    /// otherwise reached only from `handle_webhook_post`'s request path.
    /// Lets the N-02 bounding tests drive the prune directly with a
    /// `FakeClock` instead of issuing thousands of real HTTP requests.
    #[doc(hidden)]
    pub fn record_origin_callback_for_test(&self, delivery_id: String, url: String) -> bool {
        self.record_origin_callback(delivery_id, url)
    }
}

/// How long an unclaimed `deliver: origin` callback URL is retained before it
/// is pruned (security audit N-02). Generously longer than any agent turn is
/// expected to take, because expiring a callback whose turn is still running
/// would turn a slow answer into an undeliverable one — the TTL exists to
/// bound a leak, not to time turns out.
const ORIGIN_CALLBACK_TTL: std::time::Duration = std::time::Duration::from_secs(3600);

/// Hard cap on retained `deliver: origin` callback URLs, covering the case the
/// TTL cannot: sustained arrival faster than entries expire.
/// `pub` so the rail test that drives the cap arm can size its fixture from
/// the real constant rather than a hardcoded copy that could silently drift.
pub const ORIGIN_CALLBACK_MAX_ENTRIES: usize = 10_000;

/// Longest `callback_url` value [`extract_origin_callback_url`] will accept,
/// in bytes (security review WR-03).
///
/// No legitimate callback URL approaches this — it is roughly the practical
/// URL ceiling browsers and proxies converged on. It exists because
/// [`ORIGIN_CALLBACK_MAX_ENTRIES`] bounds the entry COUNT of the retained
/// callback table and nothing bounded the entry SIZE: the only other ceiling
/// was `rails.max_body_bytes` (1 MiB by default), making the worst case
/// 10_000 × 1 MiB of hour-pinned memory. An over-long value is dropped as if
/// no `callback_url` were present, which routes it into
/// `handle_webhook_post`'s existing "no callback_url" arm — a loud `warn!`
/// and a `send_message` that fails loudly, never a silent mis-delivery.
const MAX_CALLBACK_URL_BYTES: usize = 2048;

/// Resolve one route's [`Verifier`] from its selected [`SignatureKind`] and
/// named env var, per the D-14 rule that secret VALUES never live in
/// config.yaml — only the name of the environment variable they are read
/// from.
fn resolve_verifier(route: &WebhookRoute) -> Result<Verifier> {
    match route.signature {
        SignatureKind::GenericV2 => {
            let env_name = route.secret_env.as_deref().ok_or_else(|| {
                anyhow!(
                    "webhook route '{}': signature=generic_v2 requires secret_env",
                    route.name
                )
            })?;
            let secret = std::env::var(env_name).map_err(|_| {
                anyhow!(
                    "webhook route '{}': environment variable '{env_name}' (secret_env) is not set",
                    route.name
                )
            })?;
            Ok(Verifier::GenericV2 {
                secret,
                skew_secs: route.timestamp_skew_secs,
            })
        }
        SignatureKind::None => Ok(Verifier::None),
        SignatureKind::Twilio => {
            let env_name = route.auth_token_env.as_deref().ok_or_else(|| {
                anyhow!(
                    "webhook route '{}': signature=twilio requires auth_token_env",
                    route.name
                )
            })?;
            let auth_token = std::env::var(env_name).map_err(|_| {
                anyhow!(
                    "webhook route '{}': environment variable '{env_name}' (auth_token_env) is not set",
                    route.name
                )
            })?;
            Ok(Verifier::Twilio { auth_token })
        }
        SignatureKind::Telnyx => {
            let env_name = route.public_key_env.as_deref().ok_or_else(|| {
                anyhow!(
                    "webhook route '{}': signature=telnyx requires public_key_env",
                    route.name
                )
            })?;
            let public_key_raw = std::env::var(env_name).map_err(|_| {
                anyhow!(
                    "webhook route '{}': environment variable '{env_name}' (public_key_env) is not set",
                    route.name
                )
            })?;
            // Decoded into a `VerifyingKey` here — at construction time — so
            // a malformed public key fails `WebhookAdapter::new` outright
            // rather than surfacing on the first live webhook request
            // (Phase 36.7.1 Plan 02, D-09/D-10).
            Verifier::telnyx_from_env_value(&public_key_raw, route.timestamp_skew_secs).map_err(
                |e| anyhow!("webhook route '{}': {e}", route.name),
            )
        }
    }
}

/// Resolve `route.outbound_auth` into a `(header_name, header_value)` pair,
/// reading the credential VALUE from the environment at delivery time and
/// never logging it (D-14). `pub` so integration tests (`tests/webhook_delivery.rs`)
/// can assert the header-construction contract directly without needing a
/// live non-loopback HTTP target (see that file's module doc for why one
/// cannot exist in this test environment).
pub fn resolve_outbound_auth(auth: &OutboundAuth) -> Option<(String, String)> {
    match auth {
        OutboundAuth::None => None,
        OutboundAuth::Bearer { env } => std::env::var(env)
            .ok()
            .map(|token| ("Authorization".to_string(), format!("Bearer {token}"))),
        OutboundAuth::Basic { user_env, pass_env } => {
            use base64::Engine as _;
            let user = std::env::var(user_env).ok()?;
            let pass = std::env::var(pass_env).ok()?;
            let encoded =
                base64::engine::general_purpose::STANDARD.encode(format!("{user}:{pass}"));
            Some(("Authorization".to_string(), format!("Basic {encoded}")))
        }
    }
}

/// D-11 origin family: extract the caller-supplied callback URL from the
/// inbound payload under the reserved `callback_url` key (form field, or a
/// top-level JSON string field of the same name). Untrusted by
/// construction — this function does NO validation of the value; the SSRF
/// check happens exactly where every other `deliver: url`-shaped target is
/// checked (`deliver::deliver_to_url`, immediately before the POST), never
/// here, so a caller-supplied address gets identically strict treatment to
/// an operator-configured one.
fn extract_origin_callback_url(payload: &template::PayloadView<'_>) -> Option<String> {
    let raw = match payload {
        template::PayloadView::Form(form) => form.get("callback_url").cloned(),
        template::PayloadView::Json(value) => value
            .get("callback_url")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        template::PayloadView::Empty => None,
    };
    // WR-03: bound the VALUE, not just the entry count.
    // `ORIGIN_CALLBACK_MAX_ENTRIES` caps how many callback URLs are retained,
    // not how large they are, and the only other ceiling on this string is
    // `rails.max_body_bytes` (1 MiB default). 10_000 × 1 MiB is ~10 GB of
    // retained, TTL-pinned-for-an-hour memory reachable by a sender that
    // passes signature verification. Bounding here — where a 1 MiB "URL" is
    // already known to be nonsense — keeps a body-sized string out of the
    // retained map entirely, rather than admitting it and relying on the
    // entry cap.
    raw.filter(|s| s.len() <= MAX_CALLBACK_URL_BYTES)
}

#[async_trait]
impl PlatformAdapter for WebhookAdapter {
    fn platform(&self) -> Platform {
        Platform::Webhook
    }

    /// `chat_id` is the route name. Delivers per that route's configured
    /// `deliver` target (D-11: `url`, `platform`, or `origin`).
    ///
    /// For `deliver: origin`, `thread_id` carries this delivery's id — the
    /// key `handle_webhook_post` used to stash the caller-supplied callback
    /// URL in `self.origin_callbacks` when the request arrived. **Caller
    /// contract:** whatever constructs the `MessageEvent` for a webhook
    /// turn MUST thread `event.thread_id` back into this call unchanged
    /// (`ironhermes-gateway`'s `StreamConsumer` already supports this via
    /// `with_reply_to`, precedented by the existing Buzz channel-reply
    /// wiring — see this crate's top-level docs / the phase SUMMARY for the
    /// one remaining cross-crate wire this plan's file scope cannot reach).
    async fn send_message(
        &self,
        chat_id: &str,
        content: &str,
        thread_id: Option<&str>,
    ) -> Result<MessageResponse> {
        let route = self
            .config
            .routes
            .iter()
            .find(|r| r.name == chat_id)
            .ok_or_else(|| anyhow!("webhook send_message: unknown route '{chat_id}'"))?;

        let outcome = match route.deliver {
            DeliverTarget::Url => {
                let url = route
                    .deliver_url
                    .as_deref()
                    .ok_or_else(|| anyhow!("webhook route '{chat_id}' has no deliver_url"))?;
                let auth_header = resolve_outbound_auth(&route.outbound_auth);
                deliver::deliver_to_url(&self.client, url, content, auth_header).await
            }
            DeliverTarget::Platform => {
                let platform_name = route.deliver_platform.as_deref().ok_or_else(|| {
                    anyhow!("webhook route '{chat_id}' has no deliver_platform")
                })?;
                let target_chat_id = route.deliver_chat_id.as_deref().unwrap_or(chat_id);
                let registry = self.delivery_registry.read().await;
                deliver::deliver_to_platform(&registry, platform_name, target_chat_id, content)
                    .await
            }
            DeliverTarget::Origin => {
                let Some(delivery_id) = thread_id else {
                    tracing::error!(
                        route = %chat_id,
                        "webhook deliver_to_origin: no delivery id (thread_id) supplied — \
                         cannot resolve the caller-supplied callback URL for this turn"
                    );
                    return Err(anyhow!(
                        "webhook route '{chat_id}': deliver=origin turn produced no delivery \
                         id to resolve its callback URL"
                    ));
                };
                let callback_url = self
                    .origin_callbacks
                    .lock()
                    .unwrap()
                    .remove(delivery_id)
                    .map(|(url, _inserted_at)| url);
                match callback_url {
                    Some(url) => deliver::deliver_to_url(&self.client, &url, content, None).await,
                    None => {
                        tracing::error!(
                            route = %chat_id,
                            delivery_id,
                            "webhook deliver_to_origin: no callback URL recorded for this \
                             delivery (missing from the inbound payload, or already consumed)"
                        );
                        deliver::DeliveryOutcome::Failed(
                            "no callback URL recorded for this delivery".to_string(),
                        )
                    }
                }
            }
        };

        match outcome {
            deliver::DeliveryOutcome::Delivered => Ok(MessageResponse {
                message_id: uuid::Uuid::new_v4().to_string(),
                chat_id: chat_id.to_string(),
                platform: Platform::Webhook,
            }),
            deliver::DeliveryOutcome::Failed(reason) => {
                Err(anyhow!("webhook delivery to route '{chat_id}' failed: {reason}"))
            }
        }
    }

    async fn send_message_markdown_v2(
        &self,
        chat_id: &str,
        content: &str,
        thread_id: Option<&str>,
    ) -> Result<MessageResponse> {
        // No MarkdownV2 dialect for a webhook delivery target — delegate to
        // plain send_message (the trait doc explicitly permits this).
        self.send_message(chat_id, content, thread_id).await
    }

    // D-12/D-13: there is no in-place edit for an HTTP request that already
    // received its 202 response — one send is the whole delivery. This
    // no-op and `supports_in_place_edits` returning `false` directly below
    // MUST stay adjacent: a cross-AI review of phase 47.6 caught
    // `BuzzAdapter`'s equivalent pair drifting apart, which silently
    // streams a turn's answer into an edit that does nothing.
    async fn edit_message(&self, chat_id: &str, message_id: &str, _content: &str) -> Result<()> {
        tracing::info!(
            chat_id,
            message_id,
            "Webhook edit_message: no-op (the 202 response already returned; there is no \
             in-place edit for an HTTP webhook request)"
        );
        Ok(())
    }

    fn supports_in_place_edits(&self) -> bool {
        false
    }

    async fn edit_message_markdown_v2(
        &self,
        chat_id: &str,
        message_id: &str,
        content: &str,
    ) -> Result<()> {
        self.edit_message(chat_id, message_id, content).await
    }

    async fn delete_message(&self, _chat_id: &str, _message_id: &str) -> Result<()> {
        // No-op: a fire-and-forget HTTP delivery has nothing to delete.
        Ok(())
    }

    fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }
}

/// Per-route state captured by the axum handler closures `build_router`
/// installs — one per configured route path.
struct RouteState {
    route: WebhookRoute,
    adapter: Arc<WebhookAdapter>,
    handler: Arc<dyn MessageHandler>,
    cancel: CancellationToken,
}

/// Reconstruct the exact request URL the sender addressed — scheme, host,
/// path and query, with NO normalisation (no dropped default port, no
/// re-encoded query, no added/removed trailing slash). The Twilio scheme
/// (Plan 02) signs this string byte for byte. When the listener sits behind
/// a reverse proxy, `external_base_url` must be configured — otherwise the
/// base is reconstructed from the inbound `Host` header (best effort,
/// assumes `http`; an operator terminating TLS in front of this listener
/// and using a Twilio route MUST set `external_base_url`).
fn build_request_url(
    external_base_url: &Option<String>,
    uri: &axum::http::Uri,
    headers: &axum::http::HeaderMap,
) -> String {
    let path_and_query = uri
        .path_and_query()
        .map(|pq| pq.as_str())
        .unwrap_or_else(|| uri.path());
    match external_base_url {
        Some(base) => format!("{}{}", base.trim_end_matches('/'), path_and_query),
        None => {
            let host = headers
                .get(axum::http::header::HOST)
                .and_then(|v| v.to_str().ok())
                .unwrap_or("localhost");
            format!("http://{host}{path_and_query}")
        }
    }
}

/// The per-request handler installed for every configured route path.
/// Verifies, renders, and — on acceptance — returns 202 IMMEDIATELY and
/// runs the agent turn on a spawned task (D-12: webhook senders time out
/// around ten seconds and an agent turn does not fit in that window).
async fn handle_webhook_post(state: Arc<RouteState>, req: Request) -> Response {
    // D-15: the body cap is enforced as an `axum::extract::DefaultBodyLimit`
    // router layer (`build_router` below applies it per route, keyed off
    // that route's own `max_body_bytes`) rather than a manual
    // `Content-Length` header check — a chunked request carries no such
    // header, so a header check would be bypassable by construction and
    // would read the whole body before deciding. `with_limited_body` reads
    // the limit the layer stashed on the request extensions and wraps the
    // body in `http_body_util::Limited`, which errors the instant the
    // CUMULATIVE POLLED size exceeds it — streamed chunk by chunk, never by
    // trusting a declared length — so a chunked oversized body is refused
    // exactly like one carrying `Content-Length`.
    let req = req.with_limited_body();
    let max_body_bytes = state.route.rails.max_body_bytes as usize;
    let (parts, body) = req.into_parts();

    let body_bytes = match axum::body::to_bytes(body, max_body_bytes).await {
        Ok(b) => b,
        Err(_) => {
            return (
                StatusCode::PAYLOAD_TOO_LARGE,
                format!("request body exceeds the {max_body_bytes}-byte limit for this route"),
            )
                .into_response();
        }
    };

    let content_type = parts
        .headers
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    // D-14: content-type-aware body parsing. Parse
    // application/x-www-form-urlencoded with `url::form_urlencoded` — no
    // hand-written key/value splitting, no double-decoding. Every other
    // content type, including JSON, leaves the parsed-form map absent so a
    // verifier/template can tell the two body kinds apart. The raw body
    // bytes are never mutated by this branch either way.
    let parsed_form: Option<HashMap<String, String>> = if content_type
        .starts_with("application/x-www-form-urlencoded")
    {
        Some(
            url::form_urlencoded::parse(&body_bytes)
                .into_owned()
                .collect(),
        )
    } else {
        None
    };

    let request_url = build_request_url(&state.adapter.config.external_base_url, &parts.uri, &parts.headers);

    let verify_req = VerifyRequest {
        raw_body: &body_bytes,
        parsed_form: parsed_form.as_ref(),
        request_url: &request_url,
        headers: &parts.headers,
    };

    let Some(verifier) = state.adapter.verifiers.get(&state.route.name) else {
        tracing::error!(route = %state.route.name, "webhook route has no verifier configured");
        return (StatusCode::INTERNAL_SERVER_ERROR, "route misconfigured").into_response();
    };

    let outcome = verifier.verify(&verify_req);
    if !outcome.is_accepted() {
        tracing::warn!(route = %state.route.name, ?outcome, "webhook route refused request");
        return (StatusCode::UNAUTHORIZED, "unauthorized").into_response();
    }

    // D-15: the rate limiter runs AFTER verification (an unauthenticated
    // caller must never be able to exhaust a legitimate sender's budget)
    // and BEFORE the idempotency claim (a verified but excessive sender is
    // throttled before it can occupy key space it will not get to use).
    match state.adapter.rate_limiters.get(&state.route.name) {
        Some(limiter) => {
            if !limiter.admit() {
                tracing::warn!(route = %state.route.name, "webhook route rate limit exceeded");
                return (
                    StatusCode::TOO_MANY_REQUESTS,
                    format!(
                        "rate limit exceeded: max {} requests per minute for this route",
                        limiter.limit()
                    ),
                )
                    .into_response();
            }
        }
        None => {
            tracing::error!(route = %state.route.name, "webhook route has no rate limiter configured");
            return (StatusCode::INTERNAL_SERVER_ERROR, "route misconfigured").into_response();
        }
    }

    // D-15/D-17: claim the delivery's idempotency key AFTER verification —
    // a claim taken before verification would let an unauthenticated caller
    // occupy the key space for a delivery that has not arrived yet, a
    // denial of service dressed as a cache. A retried delivery that reaches
    // this point runs the agent turn exactly once; every claimant still
    // receives the same 202 acknowledgement, so a retrying sender sees
    // success rather than an error it will only retry again.
    //
    // CR-02: the key is derived from the VERIFIED signature, never from the
    // sender-supplied `X-Webhook-Idempotency-Key`. That header is unsigned
    // under every scheme, so honouring it let one captured valid request be
    // replayed with a fresh key per replay (each running a full agent turn),
    // or with a key the genuine sender would later present (silently
    // swallowing the real delivery).
    //
    // CR-02 (reopened, second pass): the header MUST be selected by THIS
    // route's own `SignatureKind`, not by scanning all three names for
    // whichever happens to be present. A verifier reads exactly one header
    // and ignores the rest — `verify_twilio` reads only `X-Twilio-Signature`,
    // `verify_telnyx` only `telnyx-signature-ed25519` — and none of them
    // rejects a request for carrying extra headers. So a fixed-order scan on
    // a `twilio`/`telnyx`/`none` route keys on a header nothing verified: an
    // attacker replays one captured, validly-signed request N times, adding
    // only a fresh `X-Webhook-Signature-V2` nonce each time, and every replay
    // derives a distinct key, claims it, and spawns a full agent turn. On a
    // `none` route the same trick skips the body-digest fallback that is the
    // route's only de-duplication.
    //
    // Matched exhaustively (no `_` arm) so a future `SignatureKind` variant
    // is a compile error here rather than a silent fallthrough onto whatever
    // header the scan reaches first.
    let signature_value = match state.route.signature {
        SignatureKind::GenericV2 => parts.headers.get(verifier::HEADER_SIGNATURE_V2),
        SignatureKind::Twilio => parts.headers.get(verifier::HEADER_TWILIO_SIGNATURE),
        SignatureKind::Telnyx => parts.headers.get(verifier::HEADER_TELNYX_SIGNATURE),
        // No authenticated material exists on this route at all, so there is
        // nothing safe to key on — fall through to `derive_key`'s route+body
        // digest, which is the correct de-duplication for an unsigned route
        // and is not attacker-selectable.
        SignatureKind::None => None,
    }
    .and_then(|v| v.to_str().ok());
    let idempotency_key =
        idempotency::derive_key(&state.route.name, signature_value, &body_bytes);
    match state.adapter.idempotency_caches.get(&state.route.name) {
        Some(cache) => {
            if !cache.claim(&idempotency_key) {
                tracing::info!(
                    route = %state.route.name,
                    "duplicate delivery de-duplicated by the idempotency cache"
                );
                return StatusCode::ACCEPTED.into_response();
            }
        }
        None => {
            tracing::error!(route = %state.route.name, "webhook route has no idempotency cache configured");
            return (StatusCode::INTERNAL_SERVER_ERROR, "route misconfigured").into_response();
        }
    }

    // Render the prompt — content-type-aware (D-14), same branch selection
    // as the parsing above.
    let json_value: Option<serde_json::Value> = if parsed_form.is_none() {
        serde_json::from_slice(&body_bytes).ok()
    } else {
        None
    };
    let payload_view = if let Some(ref form) = parsed_form {
        template::PayloadView::Form(form)
    } else if let Some(ref v) = json_value {
        template::PayloadView::Json(v)
    } else {
        template::PayloadView::Empty
    };

    let rendered = match template::render(&state.route.prompt_template, &payload_view) {
        Ok(r) => r,
        Err(e) => {
            tracing::error!(route = %state.route.name, "webhook prompt template render error: {e}");
            return (StatusCode::INTERNAL_SERVER_ERROR, "template error").into_response();
        }
    };

    // D-13: a `deliver_only` route renders and delivers — that is the whole
    // processing. No `MessageEvent` is constructed and no agent turn runs
    // (zero token cost, sub-second completion by construction), so the
    // response carries the rendered text in-body with a success status
    // rather than the 202-then-background shape below — there is nothing
    // running in the background for the caller to wait for. Placed AFTER
    // verification, the rate limiter and the idempotency claim: a
    // `deliver_only` route skips the model, not the gate.
    if state.route.deliver_only {
        let outcome = match state.route.deliver {
            DeliverTarget::Url => {
                let Some(url) = state.route.deliver_url.as_deref() else {
                    tracing::error!(route = %state.route.name, "deliver_only route has no deliver_url");
                    return (StatusCode::INTERNAL_SERVER_ERROR, "route misconfigured")
                        .into_response();
                };
                let auth_header = resolve_outbound_auth(&state.route.outbound_auth);
                deliver::deliver_to_url(&state.adapter.client, url, &rendered, auth_header).await
            }
            DeliverTarget::Platform => {
                let Some(platform_name) = state.route.deliver_platform.as_deref() else {
                    tracing::error!(route = %state.route.name, "deliver_only route has no deliver_platform");
                    return (StatusCode::INTERNAL_SERVER_ERROR, "route misconfigured")
                        .into_response();
                };
                let target_chat_id = state
                    .route
                    .deliver_chat_id
                    .as_deref()
                    .unwrap_or(&state.route.name);
                let registry = state.adapter.delivery_registry.read().await;
                deliver::deliver_to_platform(&registry, platform_name, target_chat_id, &rendered)
                    .await
            }
            DeliverTarget::Origin => match extract_origin_callback_url(&payload_view) {
                Some(url) => {
                    deliver::deliver_to_url(&state.adapter.client, &url, &rendered, None).await
                }
                None => {
                    tracing::error!(
                        route = %state.route.name,
                        "deliver_only origin route received no callback_url in payload"
                    );
                    deliver::DeliveryOutcome::Failed(
                        "deliver: origin route received no callback_url in payload".to_string(),
                    )
                }
            },
        };
        return match outcome {
            deliver::DeliveryOutcome::Delivered => (StatusCode::OK, rendered).into_response(),
            deliver::DeliveryOutcome::Failed(reason) => {
                tracing::error!(route = %state.route.name, %reason, "deliver_only delivery failed");
                (StatusCode::BAD_GATEWAY, format!("delivery failed: {reason}")).into_response()
            }
        };
    }

    // O-03: session identity for this delivery. Ephemeral (the default)
    // derives a fresh identity per delivery — route name plus a new v4
    // UUID — so two deliveries on the same route never share history.
    // Persistent reuses one stable identity (the bare route name) across
    // every delivery on the route, an explicit per-route opt-in. `chat_id`
    // stays the route name either way — `WebhookAdapter::send_message`'s
    // route lookup depends on it — only `sender_id` (which feeds
    // `SessionKey`'s `user_id` half; see `ironhermes-gateway`'s
    // `handler.rs`) varies.
    let sender_id = match state.route.session {
        SessionMode::Ephemeral => format!("{}-{}", state.route.name, uuid::Uuid::new_v4()),
        SessionMode::Persistent => state.route.name.clone(),
    };

    let message_id = uuid::Uuid::new_v4().to_string();

    // D-11 origin family: a caller-supplied callback URL travels from this
    // request's payload to the eventual `send_message` call (issued later,
    // by the external `MessageHandler`, once the agent turn completes) via
    // this adapter's own `origin_callbacks` map, keyed by this delivery's
    // message id and threaded through as the `MessageEvent`'s `thread_id`
    // (see `send_message`'s own doc for the caller contract this depends
    // on). Treated as an untrusted URL target — the exact same SSRF
    // validation any configured `deliver: url` target gets applies at
    // delivery time (`deliver::deliver_to_url`), never weakened for a
    // caller-supplied value.
    let thread_id = if matches!(state.route.deliver, DeliverTarget::Origin) {
        match extract_origin_callback_url(&payload_view) {
            Some(url) => {
                if state
                    .adapter
                    .record_origin_callback(message_id.clone(), url)
                {
                    Some(message_id.clone())
                } else {
                    // At the cap with nothing evictable (N-02). Carrying a
                    // thread_id whose entry does not exist would make
                    // `send_message` report "already consumed" — the same
                    // symptom as a genuine double-delivery. Carry none, so
                    // the failure names the real cause.
                    None
                }
            }
            None => {
                tracing::warn!(
                    route = %state.route.name,
                    "deliver: origin route received a request with no callback_url in its \
                     payload — this delivery's send_message will fail loudly rather than \
                     silently dropping the answer"
                );
                None
            }
        }
    } else {
        None
    };

    let msg_event = MessageEvent {
        platform: Platform::Webhook,
        message_id,
        chat_id: state.route.name.clone(),
        sender_id,
        content: rendered,
        attachments: vec![],
        thread_id,
        chat_type: "webhook".to_string(),
        chat_name: None,
        sender_name: None,
        replied_to_id: None,
    };

    // D-12: 202 IMMEDIATELY, turn runs on a spawned task. Mirrors
    // `run_buzz_adapter`'s dispatch exactly (buzz.rs:1251).
    let handler = state.handler.clone();
    let adapter_for_task: Arc<dyn PlatformAdapter> = state.adapter.clone();
    let child_cancel = state.cancel.child_token();
    tokio::spawn(async move {
        if let Err(e) = handler.handle(&msg_event, adapter_for_task, child_cancel).await {
            tracing::error!("webhook handler error: {e:#}");
        }
    });

    StatusCode::ACCEPTED.into_response()
}

/// Build the axum router: one `POST` route per configured route path, each
/// capturing its own [`RouteState`].
fn build_router(
    adapter: Arc<WebhookAdapter>,
    handler: Arc<dyn MessageHandler>,
    cancel: CancellationToken,
) -> axum::Router {
    let mut router = axum::Router::new();
    for route in adapter.config.routes.clone() {
        let path = route.path.clone();
        // D-15: this route's cap is applied as its OWN `DefaultBodyLimit`
        // layer, scoped to this route's `MethodRouter` before it merges
        // into the shared router (axum's own "different limits for
        // different routes" pattern) — never as a whole-router layer, which
        // would apply only the LAST-registered route's cap to every route.
        let max_body_bytes = route.rails.max_body_bytes as usize;
        let state = Arc::new(RouteState {
            route,
            adapter: adapter.clone(),
            handler: handler.clone(),
            cancel: cancel.clone(),
        });
        let method_router = axum::routing::post(move |req: Request| {
            let state = state.clone();
            async move { handle_webhook_post(state, req).await }
        })
        .layer(axum::extract::DefaultBodyLimit::max(max_body_bytes));
        router = router.route(&path, method_router);
    }
    router
}

/// Serve an already-bound listener. Split out from [`run_webhook_adapter`]
/// so tests can bind their own ephemeral `127.0.0.1:0` listener, read
/// `local_addr()`, and drive real HTTP requests against it — the same
/// ephemeral-port idiom `buzz_agent_turn.rs` uses for its relay harness.
pub async fn serve_webhook_adapter(
    listener: TcpListener,
    adapter: Arc<WebhookAdapter>,
    handler: Arc<dyn MessageHandler>,
    cancel: CancellationToken,
) -> Result<()> {
    let bound_addr = listener.local_addr().context("webhook adapter: local_addr failed")?;
    adapter.running.store(true, Ordering::SeqCst);
    tracing::info!(addr = %bound_addr, "Webhook adapter listening");

    let router = build_router(adapter.clone(), handler, cancel.clone());
    let shutdown_cancel = cancel.clone();

    let result = axum::serve(listener, router)
        .with_graceful_shutdown(async move {
            shutdown_cancel.cancelled().await;
        })
        .await;

    adapter.running.store(false, Ordering::SeqCst);
    result.map_err(|e| anyhow!("webhook adapter server error: {e}"))
}

/// The inbound loop: binds the configured listener and serves until
/// `cancel` fires. Evaluates [`crate::bind_guard::bind_guard_allows`]
/// BEFORE `TcpListener::bind` (D-07) — a refused configuration never opens
/// a socket. By the time this function runs, `WebhookAdapter::new` has
/// already enforced the same D-10 rail at construction; this re-check is
/// the defense-in-depth D-07 requires structurally, not just at
/// construction time.
pub async fn run_webhook_adapter(
    adapter: Arc<WebhookAdapter>,
    handler: Arc<dyn MessageHandler>,
    cancel: CancellationToken,
) -> Result<()> {
    let host_ip: IpAddr = adapter
        .config
        .host
        .parse()
        .with_context(|| format!("webhook adapter: invalid bind host '{}'", adapter.config.host))?;

    if !crate::bind_guard::bind_guard_allows(host_ip, !adapter.requires_loopback) {
        return Err(anyhow!(
            "webhook adapter refusing to bind {}:{} — non-loopback host with an \
             unauthenticated (signature=none) route configured (D-07)",
            adapter.config.host,
            adapter.config.port
        ));
    }

    let addr = format!("{}:{}", adapter.config.host, adapter.config.port);
    let listener = TcpListener::bind(&addr)
        .await
        .with_context(|| format!("webhook adapter failed to bind {addr}"))?;

    serve_webhook_adapter(listener, adapter, handler, cancel).await
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── WR-03: the callback URL's VALUE is bounded, not just the entry count ──

    fn json_payload(url: &str) -> serde_json::Value {
        serde_json::json!({ "callback_url": url })
    }

    #[test]
    fn a_plausible_callback_url_is_accepted() {
        let v = json_payload("https://sender.example.test/hooks/answer?id=42");
        let out = extract_origin_callback_url(&template::PayloadView::Json(&v));
        assert_eq!(
            out.as_deref(),
            Some("https://sender.example.test/hooks/answer?id=42"),
            "the bound must not reject any URL a real sender would send"
        );
    }

    /// Both sides of the boundary, so the comparison can be neither
    /// off-by-one nor silently inverted.
    #[test]
    fn the_callback_url_bound_is_inclusive_and_exact() {
        const PREFIX: &str = "https://x.test/";
        let at = format!(
            "{PREFIX}{}",
            "a".repeat(MAX_CALLBACK_URL_BYTES - PREFIX.len())
        );
        assert_eq!(
            at.len(),
            MAX_CALLBACK_URL_BYTES,
            "fixture must sit exactly ON the bound"
        );
        let v = json_payload(&at);
        assert!(
            extract_origin_callback_url(&template::PayloadView::Json(&v)).is_some(),
            "the bound is inclusive — a value of exactly MAX_CALLBACK_URL_BYTES is fine"
        );

        let over = format!("{at}a");
        let v = json_payload(&over);
        assert!(
            extract_origin_callback_url(&template::PayloadView::Json(&v)).is_none(),
            "one byte past the bound must be refused"
        );
    }

    #[test]
    fn a_body_sized_callback_url_never_enters_the_retained_map() {
        // The worst case WR-03 names: a value bounded only by
        // `rails.max_body_bytes` (1 MiB default). 10_000 of these, each pinned
        // for the hour-long TTL, is ~10 GB of retained memory reachable by a
        // sender that passes signature verification.
        let v = json_payload(&"h".repeat(1024 * 1024));
        assert!(
            extract_origin_callback_url(&template::PayloadView::Json(&v)).is_none(),
            "a 1 MiB 'URL' must be dropped at extraction, not admitted and then \
             counted against the entry cap"
        );
    }

    #[test]
    fn the_form_arm_is_bounded_too() {
        // Both body kinds reach this function; bounding only the JSON arm
        // would leave the form arm — the Twilio shape — wide open.
        let mut form = HashMap::new();
        form.insert("callback_url".to_string(), "f".repeat(1024 * 1024));
        assert!(
            extract_origin_callback_url(&template::PayloadView::Form(&form)).is_none(),
            "the form arm must be bounded identically to the JSON arm"
        );
    }

    // ── WR-02: axum panics on more than just a byte-identical duplicate ──────

    fn route_with_path(name: &str, path: &str) -> WebhookRoute {
        WebhookRoute {
            name: name.to_string(),
            path: path.to_string(),
            signature: SignatureKind::None,
            secret_env: None,
            auth_token_env: None,
            public_key_env: None,
            timestamp_skew_secs: 300,
            prompt_template: "{Body}".to_string(),
            deliver: DeliverTarget::Platform,
            deliver_url: None,
            deliver_platform: Some("teststub".to_string()),
            deliver_chat_id: None,
            deliver_only: false,
            outbound_auth: OutboundAuth::None,
            session: SessionMode::Ephemeral,
            rails: Default::default(),
        }
    }

    fn construct_with_paths(paths: &[(&str, &str)]) -> Result<WebhookAdapter> {
        let routes = paths
            .iter()
            .map(|(n, p)| route_with_path(n, p))
            .collect::<Vec<_>>();
        WebhookAdapter::new(
            WebhookRoutesConfig {
                host: "127.0.0.1".to_string(),
                port: 0,
                public_opt_in: false,
                external_base_url: None,
                routes,
            },
            Arc::new(RwLock::new(DeliveryRegistry::new())),
        )
    }

    #[test]
    fn a_path_without_a_leading_slash_is_refused_at_construction() {
        // axum-0.8.4 `path_router.rs:43` — "Paths must start with a `/`". The
        // panic would unwind out of the spawned serve task AFTER
        // `running.store(true)`, leaving `is_running() == true` with no
        // listener behind it for the process lifetime.
        let Err(err) = construct_with_paths(&[("r1", "hooks/sms")]) else {
            panic!("a path with no leading '/' must be refused");
        };
        let msg = err.to_string();
        assert!(msg.contains("r1") && msg.contains("hooks/sms"), "{msg}");
    }

    #[test]
    fn an_empty_path_is_refused_at_construction() {
        // axum-0.8.4 `path_router.rs:41` — empty path panics.
        assert!(
            construct_with_paths(&[("r1", "")]).is_err(),
            "an empty path must be refused at construction"
        );
    }

    #[test]
    fn a_v07_style_capture_segment_is_refused_at_construction() {
        // axum-0.8.4 `path_router.rs:53` — a segment starting with `:` panics.
        // This is the v0.7 capture syntax an operator migrating a config would
        // naturally write.
        assert!(
            construct_with_paths(&[("r1", "/hooks/:id")]).is_err(),
            "a ':'-prefixed capture segment must be refused at construction"
        );
    }

    #[test]
    fn conflicting_capture_paths_are_refused_at_construction() {
        // `/hooks/{a}` and `/hooks/{b}` are DISTINCT strings, so the
        // uniqueness check admits both — but `matchit` rejects the second
        // insertion as a conflict and `panic_on_err!` turns that into a panic
        // (axum-0.8.4 `path_router.rs:159`). Refusing capture segments removes
        // the whole conflict class: two literal paths conflict only when equal,
        // which the uniqueness check already covers.
        assert!(
            construct_with_paths(&[("r1", "/hooks/{a}"), ("r2", "/hooks/{b}")]).is_err(),
            "capture segments must be refused, closing the matchit-conflict panic class"
        );
    }

    #[test]
    fn ordinary_literal_paths_still_construct() {
        // The bound must not reject the shapes every real config uses,
        // including a path with a trailing segment and a nested one.
        construct_with_paths(&[("r1", "/hooks/sms"), ("r2", "/webhook/github/issues")])
            .expect("literal paths must still construct");
    }

    #[test]
    fn request_url_verbatim_with_external_base_override() {
        let headers = axum::http::HeaderMap::new();
        let uri: axum::http::Uri = "/webhook/r?a=1&b=two%20words".parse().unwrap();
        let external_base = Some("https://public.example.test".to_string());
        let url = build_request_url(&external_base, &uri, &headers);
        assert_eq!(url, "https://public.example.test/webhook/r?a=1&b=two%20words");
    }

    #[test]
    fn request_url_verbatim_no_normalisation_query_preserved() {
        let headers = axum::http::HeaderMap::new();
        // Query preserved exactly, including percent-encoding, no
        // re-ordering, no default-port stripping decisions made here.
        let uri: axum::http::Uri = "/webhook/r?z=last&a=first&raw=a%2Bb".parse().unwrap();
        let external_base = Some("http://host:9999".to_string());
        let url = build_request_url(&external_base, &uri, &headers);
        assert_eq!(url, "http://host:9999/webhook/r?z=last&a=first&raw=a%2Bb");
    }

    #[test]
    fn request_url_falls_back_to_host_header_when_no_external_base() {
        let mut headers = axum::http::HeaderMap::new();
        headers.insert(
            axum::http::header::HOST,
            "example.test:8443".parse().unwrap(),
        );
        let uri: axum::http::Uri = "/webhook/r".parse().unwrap();
        let url = build_request_url(&None, &uri, &headers);
        assert_eq!(url, "http://example.test:8443/webhook/r");
    }
}
