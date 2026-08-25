//! The REST API server adapter (`ApiServerAdapter` + `run_api_server_adapter`).
//!
//! This is the OpenAI-compatible-shaped inbound HTTP surface an operator
//! points a third-party chat client at — a second, independent listener from
//! the webhook adapter ([`crate::webhook`]), on its own port (D-04). Follows
//! the same construction/serve split [`crate::webhook::WebhookAdapter`]
//! established: this struct owns configuration, shared runtime handles and
//! the running flag; the free functions [`run_api_server_adapter`] /
//! [`serve_api_server_adapter`] own the listener.
//!
//! # Security posture (D-06/D-07/D-08)
//!
//! - **Fail closed at startup (D-06):** [`ApiServerAdapter::new`] returns
//!   `Err` when `IRONHERMES_API_SERVER_KEY` is unset or empty. The surface
//!   never defaults to open.
//! - **Loopback default, non-loopback rail (D-07):** the default bind
//!   address is loopback on port [`DEFAULT_PORT`] (8642, D-05 — matches the
//!   port target so existing client configuration carries over). A
//!   non-loopback bind additionally requires the operator's explicit
//!   `public_opt_in` — see [`api_server_bind_auth_enabled`].
//! - **One auth path (D-08):** [`auth::require_bearer`] is the only
//!   authentication this surface has. No cookie session, no per-request
//!   profile selector — see that module's doc comment.

pub mod auth;
pub mod routes;
pub mod sse;

use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;

use anyhow::{Context, Result, anyhow};
use async_trait::async_trait;
use ironhermes_core::adapter::{MessageHandler, PlatformAdapter};
use ironhermes_core::{MessageResponse, Platform};
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;

/// D-05: the port target's default port, kept so existing client
/// configuration and documentation carry over unchanged.
pub const DEFAULT_PORT: u16 = 8642;
/// The loopback interface address string used when no `host` is configured.
pub const DEFAULT_HOST: &str = "127.0.0.1";

/// Runtime input to [`ApiServerAdapter::new`] — mirrors the split
/// [`crate::webhook::route_config::WebhookRoutesConfig`] established: the
/// listener-level fields the caller (`GatewayRunner::start()`, or a test
/// harness) assembles from `PlatformGatewayConfig`. Unlike the webhook
/// adapter, this surface has no operator-configured route table — its
/// routes are fixed (assembled in [`routes`]).
#[derive(Debug, Clone, Default)]
pub struct ApiServerConfig {
    /// Bind host. `None` resolves to [`DEFAULT_HOST`] (loopback) — D-05.
    pub host: Option<String>,
    /// Bind port. `None` resolves to [`DEFAULT_PORT`] (8642) — D-05.
    pub port: Option<u16>,
    /// D-07: whether the operator explicitly opted into exposing this
    /// listener on a non-loopback address. `false` unless set.
    pub public_opt_in: bool,
}

/// Shared, process-wide runtime handles constructed ONCE in
/// `GatewayRunner::start()` and threaded into [`ApiServerAdapter::new`], so
/// Plans 06-09 (runs, sessions, chat, jobs route families) can read live
/// state without editing `runner.rs`'s construction site again (Task 3).
#[derive(Clone)]
pub struct ApiServerHandles {
    /// Process-wide turn registry (Phase 39.1) — the runs surface (Plan 06)
    /// is a projection of it.
    pub turn_registry: Arc<ironhermes_core::concurrency::TurnRegistry>,
    /// The state store backing sessions.
    pub state_store: Arc<std::sync::Mutex<ironhermes_state::StateStore>>,
    /// The cron job store, `None` when the gateway was started without one.
    pub job_store: Option<Arc<std::sync::Mutex<ironhermes_cron::JobStore>>>,
    /// Backs `/v1/models` and `/api/model/options` (Task 2).
    pub model_registry: Arc<ironhermes_core::ModelRegistry>,
    /// Backs `/v1/skills` (Task 2). `None` when the gateway has no skill
    /// registry loaded.
    pub skill_registry: Option<Arc<ironhermes_core::SkillRegistry>>,
    /// Backs `/v1/toolsets` (Task 2).
    pub tool_registry: Arc<tokio::sync::RwLock<ironhermes_tools::ToolRegistry>>,
    /// Plan 06 Task 2: the seam `POST /v1/runs/{run_id}/approval` translates
    /// onto. Deliberately the [`ironhermes_core::ApprovalGate`] TRAIT, not
    /// `ironhermes_gateway::approval::ApprovalCoordinator` directly —
    /// `ironhermes-restgw` cannot depend on `ironhermes-gateway` (that crate
    /// depends on THIS one to construct `ApiServerAdapter`; the reverse edge
    /// would cycle, exactly the reasoning `ApiServerAdapter`/`WebhookAdapter`
    /// already apply by implementing `ironhermes_core::adapter::{MessageHandler,
    /// PlatformAdapter}` instead of a gateway-owned trait). `ApprovalGate`
    /// exists in `ironhermes_core` for precisely this cross-crate need (see
    /// its own module doc). `None` when no approval mechanism is wired for
    /// this adapter instance — the approval route then fails closed
    /// (not-found, nothing to resolve against) rather than silently
    /// succeeding.
    pub approval_gate: Option<Arc<dyn ironhermes_core::ApprovalGate>>,
    /// Plan 06 Task 2/3: bridges a submitted run's producer to the SSE route
    /// (`GET /v1/runs/{run_id}/events`, Task 3) that streams it. Purely
    /// `ironhermes-restgw`-internal (no cross-crate type), unlike
    /// `approval_gate` — see [`sse::RunEventRegistry`].
    pub run_events: Arc<sse::RunEventRegistry>,
}

/// The REST API server `PlatformAdapter`. Owns the resolved listener
/// configuration, the bearer key, the shared runtime handles, and the
/// running flag. Construction does no I/O (D-06 validates only the
/// environment) — binding happens later, inside [`run_api_server_adapter`].
pub struct ApiServerAdapter {
    host: String,
    port: u16,
    public_opt_in: bool,
    api_key: String,
    handles: ApiServerHandles,
    running: Arc<AtomicBool>,
    /// Set once the listener is actually bound (inside
    /// [`serve_api_server_adapter`]) — lets a caller (production logging, or
    /// a test driving the same construct+spawn path production uses)
    /// observe the resolved ephemeral address without a second bind.
    bound_addr: Arc<OnceLock<SocketAddr>>,
}

impl ApiServerAdapter {
    /// Construction-time validation (D-06), returning `Err` rather than
    /// panicking — an API-server platform that fails to construct must not
    /// take down the whole gateway process (`GatewayRunner::start()` logs
    /// and skips it, fail-closed per platform, mirroring
    /// `WebhookAdapter::new`'s own doc comment).
    ///
    /// Resolves `IRONHERMES_API_SERVER_KEY` from the environment first,
    /// refusing when it is absent or empty — the error names the variable
    /// and never a value. Then resolves the bind address: [`DEFAULT_HOST`]
    /// on [`DEFAULT_PORT`] unless overridden by `config.host`/`config.port`.
    pub fn new(config: ApiServerConfig, handles: ApiServerHandles) -> Result<Self> {
        let api_key = std::env::var("IRONHERMES_API_SERVER_KEY")
            .ok()
            .filter(|k| !k.is_empty())
            .ok_or_else(|| {
                anyhow!(
                    "API server adapter refuses to construct: IRONHERMES_API_SERVER_KEY is not \
                     set. Set the environment variable before starting the gateway (D-06 — this \
                     surface never defaults to open)."
                )
            })?;

        let host = config
            .host
            .filter(|h| !h.is_empty())
            .unwrap_or_else(|| DEFAULT_HOST.to_string());
        let port = config.port.unwrap_or(DEFAULT_PORT);

        Ok(Self {
            host,
            port,
            public_opt_in: config.public_opt_in,
            api_key,
            handles,
            running: Arc::new(AtomicBool::new(false)),
            bound_addr: Arc::new(OnceLock::new()),
        })
    }

    /// The resolved bind host (post-default-resolution).
    pub fn bound_host(&self) -> &str {
        &self.host
    }

    /// The resolved bind port (post-default-resolution).
    pub fn bound_port(&self) -> u16 {
        self.port
    }

    /// The actual bound socket address, once the listener has opened.
    /// `None` before [`run_api_server_adapter`]/[`serve_api_server_adapter`]
    /// has bound a listener for this adapter.
    pub fn bound_addr(&self) -> Option<SocketAddr> {
        self.bound_addr.get().copied()
    }

    /// The configured bearer key. `pub(crate)` — read by [`auth`] only;
    /// never logged, never echoed in a response body (D-06).
    pub(crate) fn api_key(&self) -> &str {
        &self.api_key
    }

    /// The shared runtime handles this adapter was constructed with.
    pub(crate) fn handles(&self) -> &ApiServerHandles {
        &self.handles
    }
}

/// D-07: the REST API server's non-loopback bind requires BOTH a configured
/// key AND an explicit public opt-in — stricter than
/// [`crate::bind_guard::bind_guard_allows`]'s single-flag webhook usage.
/// Pure and side-effect-free so the full four-cell matrix is testable
/// without environment mutation or a socket.
///
/// In production `key_configured` is always `true` by the time this is
/// evaluated — [`ApiServerAdapter::new`] cannot construct without a key —
/// but the conjunction is expressed explicitly (not collapsed to
/// `public_opt_in` alone) so the D-07 requirement stays visible at the call
/// site and remains independently testable against a key-absent case.
pub fn api_server_bind_auth_enabled(key_configured: bool, public_opt_in: bool) -> bool {
    key_configured && public_opt_in
}

#[async_trait]
impl PlatformAdapter for ApiServerAdapter {
    fn platform(&self) -> Platform {
        Platform::ApiServer
    }

    /// This surface has no operator-facing "send a message to a chat_id"
    /// concept in this plan — every response this plan serves goes out
    /// through the HTTP response body of the request that triggered it.
    /// Later plans (chat completions, runs) drive their own response paths
    /// directly rather than through this trait method; it exists only so
    /// `ApiServerAdapter` satisfies `PlatformAdapter`.
    async fn send_message(
        &self,
        _chat_id: &str,
        _content: &str,
        _thread_id: Option<&str>,
    ) -> Result<MessageResponse> {
        Err(anyhow!(
            "ApiServerAdapter::send_message is not implemented — this surface responds through \
             the HTTP response of the triggering request, not an out-of-band send"
        ))
    }

    async fn send_message_markdown_v2(
        &self,
        chat_id: &str,
        content: &str,
        thread_id: Option<&str>,
    ) -> Result<MessageResponse> {
        self.send_message(chat_id, content, thread_id).await
    }

    // D-08/trait doc: there is no in-place edit for an HTTP response that
    // has already been sent — this no-op and `supports_in_place_edits`
    // returning `false` directly below MUST stay adjacent (a cross-AI
    // review of phase 47.6 caught `BuzzAdapter`'s equivalent pair drifting
    // apart; `crate::webhook::WebhookAdapter` carries the identical pair for
    // the same reason).
    async fn edit_message(&self, chat_id: &str, message_id: &str, _content: &str) -> Result<()> {
        tracing::info!(
            chat_id,
            message_id,
            "API server edit_message: no-op (the HTTP response already returned; there is no \
             in-place edit for a completed HTTP response)"
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
        Ok(())
    }

    fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }
}

/// Serve an already-bound listener. Split out from [`run_api_server_adapter`]
/// so tests can bind their own ephemeral `127.0.0.1:0` listener, read
/// `local_addr()`, and drive real HTTP requests against it — the same
/// ephemeral-port idiom `crate::webhook::serve_webhook_adapter` uses.
pub async fn serve_api_server_adapter(
    listener: TcpListener,
    adapter: Arc<ApiServerAdapter>,
    handler: Arc<dyn MessageHandler>,
    cancel: CancellationToken,
) -> Result<()> {
    let _ = &handler; // Reserved for Plans 06-09's route families; unused by Tasks 1-3's read-only routes.
    let bound_addr = listener
        .local_addr()
        .context("API server adapter: local_addr failed")?;
    let _ = adapter.bound_addr.set(bound_addr);
    adapter.running.store(true, Ordering::SeqCst);
    tracing::info!(addr = %bound_addr, "API server adapter listening");

    let router = routes::build_router(adapter.clone());
    let shutdown_cancel = cancel.clone();

    let result = axum::serve(listener, router)
        .with_graceful_shutdown(async move {
            shutdown_cancel.cancelled().await;
        })
        .await;

    adapter.running.store(false, Ordering::SeqCst);
    result.map_err(|e| anyhow!("API server adapter server error: {e}"))
}

/// The inbound loop: binds the configured listener and serves until `cancel`
/// fires. Evaluates [`crate::bind_guard::bind_guard_allows`] BEFORE
/// `TcpListener::bind` (D-07) — a refused configuration never opens a
/// socket. By the time this function runs, `ApiServerAdapter::new` has
/// already enforced D-06 (a key is configured); this bind-time check adds
/// the D-07 loopback/opt-in rail as defense-in-depth, mirroring
/// `crate::webhook::run_webhook_adapter`'s own two-layer shape.
pub async fn run_api_server_adapter(
    adapter: Arc<ApiServerAdapter>,
    handler: Arc<dyn MessageHandler>,
    cancel: CancellationToken,
) -> Result<()> {
    let host_ip: IpAddr = adapter
        .host
        .parse()
        .with_context(|| format!("API server adapter: invalid bind host '{}'", adapter.host))?;

    // The key is always configured here (construction requires it); the
    // conjunction is still expressed via `api_server_bind_auth_enabled` so
    // the D-07 "both, not either" requirement stays a visible, independently
    // testable predicate rather than a collapsed single flag.
    let auth_enabled = api_server_bind_auth_enabled(true, adapter.public_opt_in);
    if !crate::bind_guard::bind_guard_allows(host_ip, auth_enabled) {
        return Err(anyhow!(
            "API server adapter refusing to bind {}:{} — non-loopback host without an explicit \
             public opt-in (D-07). Set gateway.platforms.api_server.public_opt_in: true to \
             expose this listener beyond loopback (IRONHERMES_API_SERVER_KEY is already \
             configured).",
            adapter.host,
            adapter.port
        ));
    }

    let addr = format!("{}:{}", adapter.host, adapter.port);
    let listener = TcpListener::bind(&addr)
        .await
        .with_context(|| format!("API server adapter failed to bind {addr}"))?;

    serve_api_server_adapter(listener, adapter, handler, cancel).await
}
