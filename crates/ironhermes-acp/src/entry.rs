//! ACP stdio bootstrap (D-07: stdout is protocol-only).
//!
//! Registers the `initialize` / `session/new` / `session/prompt` handlers and serves until
//! the transport reaches clean EOF, returning `Ok(())`. Does NOT construct an `AgentRuntime`
//! itself — runtime construction belongs to `session/new` (RESEARCH Pattern 1). Owns one
//! `Arc<Mutex<AcpSessionManager>>` shared by every handler for the process lifetime.
//!
//! The tracing-subscriber-to-stderr branch that keeps this module's callers safe to run
//! over stdio lives in `ironhermes-cli/src/main.rs` (must run before the first `tracing`
//! macro call and before this module's `Stdio` transport is constructed — Pitfall 1).

use std::sync::{Arc, Mutex as StdMutex};

use agent_client_protocol::{Agent, ConnectTo, Stdio};
use ironhermes_core::{Config, ProviderResolver};
use ironhermes_state::StateStore;
use tokio::sync::Mutex as TokioMutex;

use crate::handlers;
use crate::session_manager::AcpSessionManager;

/// Register the ACP handlers and serve `transport` until it reaches clean EOF.
///
/// Generic over the transport so tests can drive this over an in-memory
/// `agent_client_protocol::Channel` duplex instead of real stdio — the harness every plan
/// 02-05 test also reuses. `state_store` is an explicit parameter (rather than opened
/// internally) so tests can point it at an isolated tempdir database instead of touching
/// the operator's real `$IRONHERMES_HOME/state.db` (Phase 36.8 plan 02 task 1).
pub async fn run_acp_over(
    config: Arc<Config>,
    resolver: Arc<ProviderResolver>,
    state_store: Arc<StdMutex<StateStore>>,
    transport: impl ConnectTo<Agent>,
) -> anyhow::Result<()> {
    let session_manager = Arc::new(TokioMutex::new(AcpSessionManager::new(
        state_store,
        config.clone(),
        resolver.clone(),
    )));
    // Plan 03 task 3: captured once here, independent of `session_manager`'s own lock —
    // see `session_cancel` module doc for why `handle_session_cancel` must never need the
    // big manager lock.
    let cancel_tokens = session_manager.lock().await.cancel_tokens();

    Agent
        .builder()
        .name("ironhermes")
        .on_receive_request(
            {
                let resolver = resolver.clone();
                async move |req: agent_client_protocol::schema::v1::InitializeRequest,
                            responder,
                            cx| {
                    handlers::handle_initialize(req, responder, cx, resolver.clone()).await
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let resolver = resolver.clone();
                async move |req: agent_client_protocol::schema::v1::AuthenticateRequest,
                            responder,
                            cx| {
                    handlers::handle_authenticate(req, responder, cx, resolver.clone()).await
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let session_manager = session_manager.clone();
                async move |req: agent_client_protocol::schema::v1::NewSessionRequest,
                            responder,
                            cx| {
                    handlers::handle_session_new(req, responder, cx, session_manager.clone()).await
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let session_manager = session_manager.clone();
                async move |req: agent_client_protocol::schema::v1::PromptRequest,
                            responder,
                            cx| {
                    handlers::handle_session_prompt(req, responder, cx, session_manager.clone())
                        .await
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let session_manager = session_manager.clone();
                async move |req: agent_client_protocol::schema::v1::LoadSessionRequest,
                            responder,
                            cx| {
                    crate::session_load::handle_session_load(
                        req,
                        responder,
                        cx,
                        session_manager.clone(),
                    )
                    .await
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_notification(
            {
                let cancel_tokens = cancel_tokens.clone();
                async move |notif: agent_client_protocol::schema::v1::CancelNotification, _cx| {
                    crate::session_cancel::handle_session_cancel(notif, cancel_tokens.clone())
                        .await
                }
            },
            agent_client_protocol::on_receive_notification!(),
        )
        // D-05: explicit catch-all fallback, registered LAST so it only ever sees a method
        // none of the typed handlers above claimed ("first handler that claims a message
        // wins" — the SDK's own handler-ordering rule). `UntypedMessage` matches any method
        // by design (its `matches_method` always returns `true`). An unrecognized request
        // gets probe.rs's `-32601` reply and the connection keeps serving; an unrecognized
        // notification (no `id`) gets no response at all, per JSON-RPC 2.0 — responding to
        // a notification would itself desync a hand-rolled client. This is explicit rather
        // than relying purely on the SDK's own documented default ("Unknown requests receive
        // Method not found automatically; unhandled notifications are ignored") so the
        // behavior stays correct even if that default ever changes upstream.
        .on_receive_dispatch(
            async move |msg: agent_client_protocol::Dispatch<
                agent_client_protocol::UntypedMessage,
                agent_client_protocol::UntypedMessage,
            >,
                        _cx| {
                match msg {
                    agent_client_protocol::Dispatch::Request(req, responder) => {
                        let error = crate::probe::probe_reply(&req.method)
                            .unwrap_or_else(agent_client_protocol::Error::method_not_found);
                        responder.respond_with_error(error)
                    }
                    agent_client_protocol::Dispatch::Notification(_notif) => Ok(()),
                    agent_client_protocol::Dispatch::Response(result, router) => {
                        router.route_with_result(result)
                    }
                }
            },
            agent_client_protocol::on_receive_dispatch!(),
        )
        .connect_to(transport)
        .await?;

    Ok(())
}

/// Run the Agent Client Protocol (ACP) stdio server for editors.
///
/// `config`/`resolver` are whatever the CLI already resolved (mirrors how `run_gateway`
/// obtains them via `build_client`). D-07: stdout carries protocol frames only for the
/// lifetime of this call — the CLI's `is_acp` subscriber branch (installed before this is
/// ever called) is what keeps `tracing` output off stdout. Opens the default
/// `$IRONHERMES_HOME/state.db` (D-10) — the same store CLI/gateway sessions use.
pub async fn run_acp(config: Arc<Config>, resolver: Arc<ProviderResolver>) -> anyhow::Result<()> {
    let state_store = Arc::new(StdMutex::new(StateStore::open_default()?));
    run_acp_over(config, resolver, state_store, Stdio::new()).await
}
