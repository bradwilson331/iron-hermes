//! Phase 49.1 Plan 07 (D-05/D-11/D-16): boots ONLY `ApiServerAdapter`'s own
//! loopback listener, bypassing `ironhermes-gateway::GatewayRunner::start()`
//! and its Telegram/Discord/Slack/Buzz bootstrap entirely.
//!
//! `ironhermes gateway --non-interactive` validates `TELEGRAM_BOT_TOKEN`
//! against the real Telegram API unconditionally on startup (observed live
//! during this plan's own harness development), independent of
//! `gateway.platforms.telegram.enabled`. D-11 forbids any outbound call to a
//! real third-party API from this phase's invasive probes, and the canary
//! token cannot pass a real auth check anyway — so the full CLI entrypoint
//! is not a usable path to a live restgw capture. This example constructs
//! `ApiServerAdapter` directly from the same public constructors
//! `GatewayRunner::start()` uses (`api_server/mod.rs`), with a fresh,
//! empty-state `StateStore`/`TurnRegistry`/`ToolRegistry` and a no-op
//! `MessageHandler` — sufficient to serve every route's response SHAPE
//! (what this workstream audits) without any inbound-platform machinery.
//!
//! Usage:
//!   IRONHERMES_HOME=<dir> IRONHERMES_API_SERVER_KEY=<key> \
//!     RESTGW_PROBE_PORT=8642 \
//!     cargo run -p ironhermes-restgw --example probe_boot
//!
//! Prints `PROBE_BOOT_READY host=<h> port=<p>` to stdout once the listener
//! is bound (before that line appears, connecting is a race). Runs until
//! killed (SIGINT/SIGTERM) — the probe script that spawns this is
//! responsible for teardown.

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use ironhermes_core::adapter::{MessageHandler, PlatformAdapter};
use ironhermes_core::{MessageEvent, MessageResponse};
use tokio_util::sync::CancellationToken;

/// D-16: this probe never originates a message of its own — it exists only
/// to let restgw's own route handlers answer real HTTP requests the probe
/// script sends directly. A `MessageHandler` is still required by
/// [`ironhermes_restgw::api_server::run_api_server_adapter`]'s signature
/// (it is the handler restgw would invoke for an *inbound* chat message
/// arriving over `/v1/chat/completions` etc.) — kept trivially inert.
struct NoopHandler;

#[async_trait]
impl MessageHandler for NoopHandler {
    async fn handle(
        &self,
        _event: &MessageEvent,
        _adapter: Arc<dyn PlatformAdapter>,
        _cancel: CancellationToken,
    ) -> anyhow::Result<()> {
        Ok(())
    }
}

#[allow(dead_code)]
fn _unused_message_response_reference(_r: MessageResponse) {}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let port: u16 = std::env::var("RESTGW_PROBE_PORT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(ironhermes_restgw::api_server::DEFAULT_PORT);

    let home = std::env::var("IRONHERMES_HOME")
        .expect("IRONHERMES_HOME must be set (this probe never falls back to a real home)");
    let state_db = PathBuf::from(&home).join("state.db");

    let config = ironhermes_restgw::api_server::ApiServerConfig {
        host: Some("127.0.0.1".to_string()),
        port: Some(port),
        public_opt_in: false,
    };

    let handles = ironhermes_restgw::api_server::ApiServerHandles {
        turn_registry: Arc::new(ironhermes_core::concurrency::TurnRegistry::new()),
        state_store: Arc::new(std::sync::Mutex::new(ironhermes_state::StateStore::new(
            &state_db,
        )?)),
        job_store: None,
        model_registry: Arc::new(ironhermes_core::ModelRegistry::new()),
        skill_registry: None,
        tool_registry: Arc::new(tokio::sync::RwLock::new(
            ironhermes_tools::ToolRegistry::new(),
        )),
        approval_gate: None,
        run_events: Arc::new(ironhermes_restgw::api_server::sse::RunEventRegistry::new()),
    };

    let adapter = Arc::new(ironhermes_restgw::api_server::ApiServerAdapter::new(
        config, handles,
    )?);
    println!(
        "PROBE_BOOT_READY host={} port={}",
        adapter.bound_host(),
        adapter.bound_port()
    );

    let cancel = CancellationToken::new();
    let handler: Arc<dyn MessageHandler> = Arc::new(NoopHandler);
    ironhermes_restgw::api_server::run_api_server_adapter(adapter, handler, cancel).await
}
