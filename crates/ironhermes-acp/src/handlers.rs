//! JSON-RPC method handlers for the `ironhermes acp` server.
//!
//! Task 1 proved one path end-to-end: `initialize` -> `session/new` -> `session/prompt`
//! over a real, per-session `AgentRuntime` (RESEARCH Pattern 1). Task 2 completes the
//! `initialize` handshake against COVERAGE.md's INTEGRATE set, the two-entry `authMethods`
//! array (D-09), and D-17's client-MCP refusal, and wires `session/prompt` through the
//! full event bridge (CLI-05). Task 3 gives each turn a fresh cancellation token
//! (`session_cancel` owns the `session/cancel` handler itself). Plan 04 task 3 populates
//! `TurnRequest.approval_gate`/`terminal_intercept`/`execute_code_intercept` — every
//! `terminal`/`execute_code` call in an ACP turn now routes through
//! `ironhermes_hooks::execute_gated_command` + `DangerousCommandGuardrail` +
//! `AcpApprovalGate` (D-15).

use std::collections::HashSet;
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::schema::v1::{
    AgentCapabilities, AuthMethod, AuthMethodAgent, AuthenticateRequest, AuthenticateResponse,
    ContentBlock, Implementation, InitializeRequest, InitializeResponse, McpServer,
    NewSessionRequest, NewSessionResponse, PromptCapabilities, PromptRequest, PromptResponse,
    SessionCapabilities, SessionCloseCapabilities, SessionDeleteCapabilities,
    SessionListCapabilities, SessionUpdate, StopReason as AcpStopReason, UsageUpdate,
};
use agent_client_protocol::{Client, ConnectionTo, Responder};
use ironhermes_agent::agent_loop::{AgentResult, StopReason as HermesStopReason};
use ironhermes_agent::TurnRequest;
use ironhermes_core::{ChatMessage, MessageContent, ProviderResolver, Role as ChatRole};
use tokio::sync::Mutex as TokioMutex;
use tokio_util::sync::CancellationToken;

use crate::approval_bridge::{AcpApprovalGate, PermissionRequestSender};
use crate::event_bridge::AcpEventBridge;
use crate::session_manager::AcpSessionManager;

/// Auth method id for "the resolver already has a usable provider credential" (D-09).
const AUTH_METHOD_RESOLVED_PROVIDER: &str = "resolved-provider";
/// Auth method id for "run `ironhermes setup` in a terminal" — always advertised so
/// `authMethods` is never empty even when no credential resolves (D-09).
const AUTH_METHOD_TERMINAL: &str = "terminal";

/// Returns true when the resolver has an actually-usable credential for the configured
/// (main) provider — i.e. `resolved-provider` is a real, not aspirational, auth method.
fn has_resolved_provider_credential(resolver: &ProviderResolver) -> bool {
    resolver
        .resolve(resolver.main_provider())
        .is_some_and(|endpoint| endpoint.api_key.is_some())
}

/// `initialize` — advertises exactly COVERAGE.md's INTEGRATE set and nothing it marks
/// OPT-OUT: `load_session`; prompt capabilities `image` + `embedded_context` (audio stays
/// off — no audio-input path into `run_turn`'s message vector); session capabilities
/// `list`/`delete`/`close` (`resume`/`additional_directories` stay off — D-11/D-12); MCP
/// capabilities and auth `logout` stay at their `false`/`None` defaults (OPT-OUT). D-09:
/// `resolved-provider` is only advertised when the resolver reports a usable credential;
/// `terminal` is always present. D-05: the response always negotiates `ProtocolVersion::V1`
/// — the only stable version this agent speaks — regardless of what the client requested;
/// a bare-integer wire value (e.g. `2`) round-trips through `ProtocolVersion`'s own
/// deserializer without any coercion shim (it deserializes a raw `u16` natively).
pub async fn handle_initialize(
    req: InitializeRequest,
    responder: Responder<InitializeResponse>,
    _cx: ConnectionTo<Client>,
    resolver: Arc<ProviderResolver>,
) -> Result<(), agent_client_protocol::Error> {
    let _ = req.protocol_version; // negotiated version is always V1 (see doc comment above)

    let capabilities = AgentCapabilities::new()
        .load_session(true)
        .prompt_capabilities(PromptCapabilities::new().image(true).embedded_context(true))
        .session_capabilities(
            SessionCapabilities::new()
                .list(SessionListCapabilities::new())
                .delete(SessionDeleteCapabilities::new())
                .close(SessionCloseCapabilities::new()),
        );

    let mut auth_methods = Vec::new();
    if has_resolved_provider_credential(&resolver) {
        auth_methods.push(AuthMethod::Agent(AuthMethodAgent::new(
            AUTH_METHOD_RESOLVED_PROVIDER,
            "Configured provider credentials",
        )));
    }
    auth_methods.push(AuthMethod::Agent(AuthMethodAgent::new(
        AUTH_METHOD_TERMINAL,
        "Run `ironhermes setup`",
    )));

    let response = InitializeResponse::new(ProtocolVersion::V1)
        .agent_capabilities(capabilities)
        .auth_methods(auth_methods)
        .agent_info(Implementation::new("ironhermes", env!("CARGO_PKG_VERSION")));

    responder.respond(response)
}

/// `authenticate` — D-09: `resolved-provider` is a no-op success when credentials
/// actually resolve (fails loudly otherwise rather than lying); `terminal` always returns
/// a JSON-RPC error naming the setup command, since there is nothing this process can do
/// on the client's behalf to run an interactive terminal wizard.
pub async fn handle_authenticate(
    req: AuthenticateRequest,
    responder: Responder<AuthenticateResponse>,
    _cx: ConnectionTo<Client>,
    resolver: Arc<ProviderResolver>,
) -> Result<(), agent_client_protocol::Error> {
    match req.method_id.0.as_ref() {
        AUTH_METHOD_RESOLVED_PROVIDER => {
            if has_resolved_provider_credential(&resolver) {
                responder.respond(AuthenticateResponse::new())
            } else {
                responder.respond_with_error(agent_client_protocol::Error::auth_required().data(
                    "no provider credential is currently configured; run `ironhermes setup`",
                ))
            }
        }
        AUTH_METHOD_TERMINAL => responder.respond_with_error(
            agent_client_protocol::Error::auth_required().data(
                "run `ironhermes setup` in a terminal to configure provider credentials, \
                 then reconnect",
            ),
        ),
        other => responder.respond_with_error(
            agent_client_protocol::Error::invalid_params()
                .data(format!("unknown auth method id: {other}")),
        ),
    }
}

/// `session/new` — allocates a session id and builds a fresh per-session `AgentRuntime`
/// rooted at the client-supplied `cwd` (RESEARCH Pattern 1 / Pitfall 2: `AgentRuntime.cwd`
/// and its context-file discovery are frozen at construction, so a shared per-process
/// runtime would leak the FIRST session's project root into every later session).
/// Delegates the actual construction to `AcpSessionManager::create` (plan 02 task 1) so
/// `session/new`, `session/load`, and `fork` all share one code path.
///
/// D-17: client-supplied `mcpServers` are logged (one `tracing::warn!` per session, naming
/// the count and server names) and otherwise completely ignored — the advertised
/// capabilities already tell the client MCP passthrough is unsupported (`McpCapabilities`
/// stays at its all-`false` default), so this is the belt-and-braces refusal. No process
/// spawn, no connection attempt, no config file write; the source-level guard for this is
/// task 3's negative grep for `Command::new` across the handler path.
pub async fn handle_session_new(
    req: NewSessionRequest,
    responder: Responder<NewSessionResponse>,
    _cx: ConnectionTo<Client>,
    session_manager: Arc<TokioMutex<AcpSessionManager>>,
) -> Result<(), agent_client_protocol::Error> {
    if !req.mcp_servers.is_empty() {
        let names: Vec<&str> = req.mcp_servers.iter().map(mcp_server_name).collect();
        tracing::warn!(
            count = req.mcp_servers.len(),
            servers = ?names,
            "session/new requested client-supplied MCP servers; IronHermes ACP does not \
             support MCP passthrough (D-17) — logged and ignored, nothing spawned"
        );
    }

    let cwd = req.cwd.clone();

    let session_id = match session_manager.lock().await.create(cwd, None).await {
        Ok(id) => id,
        Err(err) => {
            return responder.respond_with_error(
                agent_client_protocol::Error::internal_error()
                    .data(format!("building AcpSession: {err}")),
            );
        }
    };

    responder.respond(NewSessionResponse::new(session_id))
}

/// `session/prompt` — flattens the ACP prompt content blocks into a single user
/// `ChatMessage`, runs one `AgentRuntime::run_turn` with the full `TurnRequest` wired
/// (event bridge, cancellation, trajectory writer, state store), streams `session/update`
/// notifications back as the turn produces text/tool activity, and reports a usage update
/// plus a truthful stop reason (CLI-05, D-18, D-10). Permission requests are plan 04's job.
///
/// Plan 03 task 3: the actual turn runs via `cx.spawn` — OUTSIDE the connection's dispatch
/// loop — rather than being awaited inline in this handler. Per the SDK's own ordering
/// contract (`agent_client_protocol::concepts::ordering`), an `on_receive_request` callback
/// "blocks further message processing until it completes"; `run_turn` can take arbitrarily
/// long (a real model call), so awaiting it inline here would mean a `session/cancel`
/// notification for THIS SAME turn could never be dispatched — and hence could never fire
/// the token — until the turn had already finished on its own, defeating cancellation
/// entirely. Spawning lets the dispatch loop keep serving `session/cancel` (and any other
/// incoming message) while the turn runs.
pub async fn handle_session_prompt(
    req: PromptRequest,
    responder: Responder<PromptResponse>,
    cx: ConnectionTo<Client>,
    session_manager: Arc<TokioMutex<AcpSessionManager>>,
) -> Result<(), agent_client_protocol::Error> {
    cx.clone().spawn(async move {
        run_prompt_turn(req, responder, cx, session_manager).await;
        Ok(())
    })
}

/// The actual `session/prompt` turn: looks the session up, builds and runs the full
/// `TurnRequest`, and responds. Moved out of `handle_session_prompt` so it can run via
/// `cx.spawn` outside the dispatch loop (see that function's doc comment). Always sends
/// SOME response on every path (`let _ =` on the terminal `responder.respond*` calls below
/// is deliberate best-effort — the SDK's own notification-delivery pattern elsewhere in
/// this crate follows the same "log, don't propagate" contract for a channel that may
/// already be closed).
async fn run_prompt_turn(
    req: PromptRequest,
    responder: Responder<PromptResponse>,
    cx: ConnectionTo<Client>,
    session_manager: Arc<TokioMutex<AcpSessionManager>>,
) {
    let session_id = req.session_id.to_string();
    let text = flatten_prompt_content(&req.prompt);

    let mut mgr = session_manager.lock().await;
    if mgr.get(&session_id).is_none() {
        drop(mgr);
        let _ = responder.respond_with_error(
            agent_client_protocol::Error::invalid_params()
                .data(format!("unknown session: {session_id}")),
        );
        return;
    }

    // Fetch the shared handles BEFORE taking a mutable borrow of the session itself
    // (`get_or_create_trajectory_writer` needs `&mut self` on the manager).
    let state_store = mgr.state_store();
    let resolver = mgr.resolver();
    let trajectory_writer = mgr.get_or_create_trajectory_writer(&session_id);
    let cancel_tokens = mgr.cancel_tokens();
    let config = mgr.config();

    let Some(session) = mgr.get_mut(&session_id) else {
        drop(mgr);
        let _ = responder.respond_with_error(
            agent_client_protocol::Error::internal_error()
                .data(format!("session {session_id} vanished mid-prompt")),
        );
        return;
    };

    // T-36.8-16: a fresh child token per turn — `CancellationToken::cancel` is terminal,
    // so reusing one token for the session's whole life would mean a single cancelled
    // turn permanently disables every later prompt on this session. Replacing it here,
    // BEFORE the turn starts, also means a cancel notification that arrives (or arrived)
    // when nothing was in flight cannot poison the turn about to start. Also refresh the
    // lock-cheap `cancel_tokens` mirror `session/cancel` reads from (see `session_cancel`
    // module doc) — `session/cancel` can only ever fire the CURRENT turn's token if this
    // mirror stays in sync with `session.cancel_token`.
    session.cancel_token = CancellationToken::new();
    let cancel_token = session.cancel_token.clone();
    cancel_tokens
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(session_id.clone(), cancel_token.clone());

    let user_message = ChatMessage {
        role: ChatRole::User,
        content: Some(MessageContent::text(text)),
        tool_calls: None,
        tool_call_id: None,
        name: None,
        is_recall_context: false,
    };
    let messages = vec![user_message.clone()];

    // Plan 04 (D-14/D-15): a fresh per-turn AcpApprovalGate bound to THIS session's own
    // `connection`/`approvals` store — never a process-wide singleton (RESEARCH Pitfall
    // 5). `client_supports_permissions: true` — the pinned ACP schema advertises no
    // explicit "permission capability" flag (unlike `terminal`/`fs`); `session/
    // request_permission` is a universally-expected client method, not an opt-in
    // extension.
    let approval_gate: Arc<dyn ironhermes_core::ApprovalGate> = Arc::new(
        AcpApprovalGate::new(
            Arc::new(cx.clone()) as Arc<dyn PermissionRequestSender>,
            session_id.clone(),
            session.approvals.clone(),
            true,
            Duration::from_secs(config.approvals.timeout_secs),
        )
        // Plan 05 (D-16): binds this turn's write_file/patch permission requests to the
        // session's cwd so the diff rendered in the permission subject matches the one
        // shown in the tool_call update (T-36.8-25).
        .with_cwd(session.cwd.clone()),
    );

    // Plan 04 task 3 (T-36.8-17/T-36.8-18, D-15): route LLM-issued `terminal` and
    // `execute_code` calls through `ironhermes_hooks::execute_gated_command` — the SAME
    // guardrail->approval->audit chokepoint the gateway uses (Phase 36.3.12) — so both
    // tools are classified by the existing `DangerousCommandGuardrail` (never an
    // ACP-local pattern list) and gated by `approval_gate` above.
    let terminal_tool = session.runtime.terminal_tool_arc();
    let execute_code_tool = session.runtime.execute_code_tool_arc();

    // D-12: the structural half of the policy-denial signal. Shared by the four gate sites
    // below (which record a denial) and the event bridge (which reads it back to decide
    // whether the client sees the "was not executed" headline), so that decision is never
    // inferred from tool output the model can influence. Fresh per turn.
    let denials = DenialLedger::new();

    let terminal_intercept: Option<ironhermes_tools::registry::InterceptHandler> = {
        let sid = session_id.clone();
        let gate = approval_gate.clone();
        let dcfg = config.dangerous_commands.clone();
        let audit_cfg = config.audit.clone();
        let yolo = config.autonomous.yolo;
        let tool = terminal_tool.clone();
        let denials = denials.clone();
        Some(Arc::new(move |args: serde_json::Value| {
            let cmd = args
                .get("command")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let sid = sid.clone();
            let gate = gate.clone();
            let guard = ironhermes_hooks::DangerousCommandGuardrail::from_config(&dcfg);
            let audit_log = ironhermes_core::AuditLog::load(audit_cfg.clone());
            let tool = tool.clone();
            let denials = denials.clone();
            Box::pin(async move {
                let outcome = ironhermes_hooks::execute_gated_command(
                    "terminal",
                    &cmd,
                    &guard,
                    Some(gate.as_ref()),
                    &audit_log,
                    &sid,
                    "acp",
                    &sid,
                    yolo,
                    false, // is_remote_backend: ACP `terminal` always runs locally
                    false, // forward_env_nonempty: ACP never forwards credentials cross-boundary
                    || async move {
                        match tool {
                            Some(t) => t.execute(args).await,
                            None => Err(anyhow::anyhow!(
                                "terminal tool not registered on this runtime"
                            )),
                        }
                    },
                )
                .await;
                map_gated_outcome("terminal", outcome, &denials)
            })
        }))
    };

    let execute_code_intercept: Option<ironhermes_tools::registry::InterceptHandler> = {
        let sid = session_id.clone();
        let gate = approval_gate.clone();
        let dcfg = config.dangerous_commands.clone();
        let audit_cfg = config.audit.clone();
        let yolo = config.autonomous.yolo;
        let tool = execute_code_tool.clone();
        let denials = denials.clone();
        Some(Arc::new(move |args: serde_json::Value| {
            let sid = sid.clone();
            let gate = gate.clone();
            let guard = ironhermes_hooks::DangerousCommandGuardrail::from_config(&dcfg);
            let audit_log = ironhermes_core::AuditLog::load(audit_cfg.clone());
            let tool = tool.clone();
            let denials = denials.clone();
            Box::pin(async move {
                let outcome = ironhermes_hooks::execute_gated_command(
                    "execute_code",
                    "", // D-11: opaque — Python source is not shell syntax
                    &guard,
                    Some(gate.as_ref()),
                    &audit_log,
                    &sid,
                    "acp",
                    &sid,
                    yolo,
                    // D-15: ACP treats EVERY execute_code call as a dangerous op requiring
                    // a permission request, closing the known cross-surface guardrail/
                    // audit bypass at THIS surface. The empty classify_arg above always
                    // classifies `Allow`, so `is_remote_backend: true` is deliberately
                    // repurposed as the existing D-08 forced-approval lever to route
                    // every call through the permission bridge regardless of
                    // classification — NOT a real backend-remoteness claim (execute_code
                    // always runs in the local sandbox); the dangerous-op classification
                    // itself still comes from `DangerousCommandGuardrail`, unchanged.
                    true,
                    false, // forward_env_nonempty: execute_code never forwards credentials cross-boundary
                    || async move {
                        match tool {
                            Some(t) => t.execute(args).await,
                            None => Err(anyhow::anyhow!(
                                "execute_code tool not registered on this runtime"
                            )),
                        }
                    },
                )
                .await;
                map_gated_outcome("execute_code", outcome, &denials)
            })
        }))
    };

    // Plan 05 (D-16, CLI-08 boundary contract): every `write_file`/`patch` call is
    // approval-gated with its diff attached, and a write whose canonicalized target
    // resolves outside the session cwd is additionally treated as a dangerous operation
    // (routed through the SAME approval_gate, with the outside-workspace reason stated in
    // the description). Registered directly on the registry (not via `TurnRequest`'s
    // hardcoded `terminal_intercept`/`execute_code_intercept` fields — no third such field
    // exists) BEFORE `run_turn`; the existing WR-05 cleanup below (triggered because
    // `terminal_intercept`/`execute_code_intercept` are always `Some`) evicts EVERY
    // intercepted name under this `session_id`, including these two, once the turn
    // completes — `unregister_intercepts_for_session` sweeps by session_id across all
    // intercepted tool names, not just terminal/execute_code.
    //
    // Plan 07 (CR-01 fix): on turn 2+ of the same session, the name has already been
    // moved out of the registry's `tools` map (turn 1's `register_intercepted_or_replace`)
    // AND swept from `intercepts` (turn 1's own end-of-turn WR-05 cleanup below), so a
    // fresh per-turn registry lookup here would find nothing and an approved write would
    // silently never reach disk. `AgentRuntime` captures both Arcs ONCE at construction
    // (mirroring `terminal_tool_arc()`/`execute_code_tool_arc()`), so these are cheap
    // synchronous clones of an already-configured tool instance, not a registry read.
    let write_file_tool_arc = session.runtime.write_file_tool_arc();
    let patch_tool_arc = session.runtime.patch_tool_arc();

    let write_file_intercept: ironhermes_tools::registry::InterceptHandler = {
        let sid = session_id.clone();
        let gate = approval_gate.clone();
        let cwd = session.cwd.clone();
        let tool = write_file_tool_arc.clone();
        let denials = denials.clone();
        Arc::new(move |args: serde_json::Value| {
            Box::pin(gate_workspace_write(
                "write_file",
                sid.clone(),
                gate.clone(),
                cwd.clone(),
                tool.clone(),
                args,
                denials.clone(),
            ))
        })
    };

    let patch_intercept: ironhermes_tools::registry::InterceptHandler = {
        let sid = session_id.clone();
        let gate = approval_gate.clone();
        let cwd = session.cwd.clone();
        let tool = patch_tool_arc.clone();
        let denials = denials.clone();
        Arc::new(move |args: serde_json::Value| {
            Box::pin(gate_workspace_write(
                "patch",
                sid.clone(),
                gate.clone(),
                cwd.clone(),
                tool.clone(),
                args,
                denials.clone(),
            ))
        })
    };

    {
        let mut reg = session.runtime.registry().write().await;
        reg.register_intercepted_or_replace(
            "write_file",
            &session_id,
            write_file_fallback_schema(),
            write_file_intercept,
        );
        reg.register_intercepted_or_replace(
            "patch",
            &session_id,
            patch_fallback_schema(),
            patch_intercept,
        );
    }

    let (bridge, drain_handle) = AcpEventBridge::new(
        Arc::new(cx.clone()),
        req.session_id.clone(),
        session.cwd.clone(),
        denials.clone(),
    );
    let turn_request = TurnRequest {
        messages,
        session_id: session_id.clone(),
        cancel_token: Some(cancel_token),
        stream: Some(bridge.stream_callback()),
        tool_progress: Some(bridge.tool_progress_callback()),
        tool_result: Some(bridge.tool_result_callback()),
        trajectory_writer: trajectory_writer.clone(),
        state_store: Some(state_store.clone()),
        approval_gate: Some(approval_gate),
        terminal_intercept,
        execute_code_intercept,
        ..Default::default()
    };

    let result = session.runtime.run_turn(turn_request).await;
    drop(mgr);

    // Resolve the outcome (stop reason or error message) BEFORE sending the response —
    // the Ok arm also persists this turn's messages and enqueues the usage update through
    // `bridge`.
    let outcome: Result<AcpStopReason, String> = match result {
        Ok(agent_result) => {
            // D-10: persist this turn's user + assistant/tool messages so `session/load`
            // has something to rehydrate and the conversation is reachable by FTS5 search.
            // `agent_result.appended` (not a role-filter over `agent_result.messages`) is
            // the round-trip output this run produced — assistant turns + matching tool
            // results, in order (see `AgentResult::appended` doc).
            match state_store.lock() {
                Ok(mut store) => {
                    if let Err(e) = store.add_message(&session_id, &user_message) {
                        tracing::warn!(error = %e, session_id = %session_id, "ACP prompt: failed to persist user message");
                    }
                    for msg in &agent_result.appended {
                        if let Err(e) = store.add_message(&session_id, msg) {
                            tracing::warn!(error = %e, session_id = %session_id, "ACP prompt: failed to persist appended message");
                        }
                    }
                }
                Err(_) => {
                    tracing::warn!(session_id = %session_id, "ACP prompt: state store lock poisoned; skipping message persistence");
                }
            }

            // Usage update, sent through the SAME channel the bridge used so it preserves
            // ordering relative to whatever stream/tool-call updates are still draining.
            let context_length = resolver.resolve_for_main().context_length();
            let used_tokens = agent_result.total_usage.total_tokens as u64;
            bridge.send_update(SessionUpdate::UsageUpdate(UsageUpdate::new(
                used_tokens,
                context_length as u64,
            )));

            Ok(map_stop_reason(&agent_result))
        }
        Err(err) => Err(err.to_string()),
    };

    // Flush: drop `bridge` (closing the channel) and await the drain task's completion so
    // every notification queued above has actually reached the wire BEFORE the response
    // goes out. Without this, `responder.respond(...)` below races the drain task — it
    // runs on a separately spawned task that is not guaranteed to have been polled yet,
    // so the response could reach the client before still-queued session/update
    // notifications, breaking the "stream/tool-call/usage arrive in order" contract.
    drop(bridge);
    if let Err(e) = drain_handle.await {
        tracing::warn!(error = %e, session_id = %session_id, "ACP event bridge drain task panicked");
    }

    match outcome {
        Ok(stop_reason) => {
            let _ = responder.respond(PromptResponse::new(stop_reason));
        }
        Err(msg) => {
            let _ = responder
                .respond_with_error(agent_client_protocol::Error::internal_error().data(msg));
        }
    }
}

/// Plan 02 (D-12): stable, self-describing marker prepended to every policy-denial error
/// text at the two ACP gate sites (`map_gated_outcome`'s `Denied`/`Blocked` arms and
/// `gate_workspace_write`'s non-approved arm). Distinguishes "this tool call was refused by
/// policy" from "this tool ran and failed" — both the model-facing tool-result text and the
/// client-facing `tool_call_update` content need this distinction (see
/// `tool_render::render_denial_content`).
///
/// ASCII only, no `"` character: `agent_loop`'s intercept wrapper JSON-encodes this text by
/// hand-replacing `"` with `'` before it reaches the client (see
/// `ironhermes-agent/src/agent_loop.rs`'s `dispatch_intercepts` branch), so a marker
/// containing a quote would not survive verbatim and a substring match downstream would
/// silently stop matching.
pub const POLICY_DENIAL_PREFIX: &str = "not run - denied by IronHermes approval policy: ";

/// Escapes `text` so it survives the JSON encoding the downstream intercept wrapper applies
/// to an intercepted tool's error.
///
/// `ironhermes-agent::agent_loop` builds that result by hand as
/// `format!(r#"{{"error":"intercept_failed","reason":"{}"}}"#, e.to_string().replace('"', "'"))`.
/// Replacing `"` is the ONLY escaping it applies, so a backslash, newline, tab or other
/// control character anywhere in the reason produces a malformed JSON string. It fails
/// asymmetrically and is therefore easy to miss: the client still renders a correct-looking
/// card (nothing on that path parses the text), while the MODEL is handed unparseable
/// garbage.
///
/// Both ACP gate sites interpolate model-influenced text into their reason — a
/// `write_file`/`patch` path, which is `C:\Users\...` on Windows (`\U` is not a valid JSON
/// escape, and this phase actively maintains Windows support) and can contain a literal
/// newline on any platform since the model chooses it, plus a guardrail reason that quotes
/// the offending command. The encoder itself is pre-existing and outside this phase's fence,
/// so the escaping belongs here, where the reason is produced.
///
/// Escaping rather than substituting means the model decodes the ORIGINAL text: a Windows
/// path arrives as the real path rather than a slash-swapped approximation. `"` is
/// deliberately left alone — the downstream transform owns that character, and doubling up
/// on it would corrupt the result.
fn encoder_safe_reason(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '\\' => out.push_str(r"\\"),
            '\n' => out.push_str(r"\n"),
            '\r' => out.push_str(r"\r"),
            '\t' => out.push_str(r"\t"),
            c if c.is_control() => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

/// Plan 47.7 (D-12): per-turn record of which tool calls THIS crate's approval gate refused.
///
/// The client-facing "BLOCKED - this tool call was not executed." headline is a claim about
/// EXECUTION, so it must only ever appear for a call that genuinely did not run. Deciding
/// that from the tool's own output text cannot be trusted: the model chooses tool arguments,
/// most tool errors echo their arguments back, and an intercepted tool's captured stderr
/// flows into the same error string — so any check over displayable output is a check over
/// content the model can influence, and a planted [`POLICY_DENIAL_PREFIX`] would render an
/// executed call as refused.
///
/// The two ACP gate sites ([`map_gated_outcome`] and [`gate_workspace_write`]) are the only
/// code that can decide a policy denial, so they record it here and the event bridge reads
/// it back when rendering that tool's terminal result. Nothing a model emits can write to
/// this ledger.
///
/// [`POLICY_DENIAL_PREFIX`] stays in the error text regardless: that is the MODEL-facing
/// half of D-12, and the model needs "refused by policy" to remain distinguishable from
/// "ran and failed" in the text it actually reads.
///
/// Scope is one turn — `handle_session_prompt` builds a fresh ledger alongside the event
/// bridge, and both are dropped when the turn ends.
#[derive(Clone, Default)]
pub struct DenialLedger {
    denied: Arc<StdMutex<HashSet<String>>>,
}

impl DenialLedger {
    pub fn new() -> Self {
        Self::default()
    }

    /// Marks the in-flight call to `tool_name` as refused by policy. Called from a gate site
    /// immediately before it returns the denial error.
    pub fn record(&self, tool_name: &str) {
        self.denied
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(tool_name.to_string());
    }

    /// Consumes the record for `tool_name`, reporting whether that call was refused by
    /// policy. The agent loop fires `tool_result` for every dispatched call (including
    /// intercepted ones), so each record is taken by the result of the very call that
    /// produced it.
    pub fn take(&self, tool_name: &str) -> bool {
        self.denied
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(tool_name)
    }
}

/// Plan 04 task 3 (D-15): translate a `GatedOutcome` into what the intercept handler
/// returns to the agent loop. `Ran` is the only success path. `Denied`/`Blocked` are
/// policy outcomes — the reason is prefixed with [`POLICY_DENIAL_PREFIX`] (D-12) so a
/// client/model can tell "refused by policy" apart from "ran and failed". `Failed` keeps
/// its current unprefixed text: a tool that ran and blew up must never be mislabelled as a
/// policy decision. Every non-`Ran` arm becomes an `Err` so the tool call surfaces as an
/// explanatory TOOL ERROR to the model and (via the event bridge's `tool_result` callback)
/// a FAILED `tool_call_update` to the client, per the plan's explicit instruction ("return
/// an explanatory tool error... so the turn continues coherently instead of the model
/// silently retrying"). This is deliberately different from the gateway's own
/// `Ok(outcome.to_string())` pattern (Phase 36.3.12), which folds every resolution into a
/// "successful" string result — ACP needs the failure to be structurally distinguishable,
/// not just textually described.
fn map_gated_outcome(
    tool_name: &str,
    outcome: ironhermes_hooks::GatedOutcome,
    denials: &DenialLedger,
) -> anyhow::Result<String> {
    match outcome {
        ironhermes_hooks::GatedOutcome::Ran(output) => Ok(output),
        ironhermes_hooks::GatedOutcome::Denied(reason)
        | ironhermes_hooks::GatedOutcome::Blocked(reason) => {
            // Record the denial structurally as well as textually: the text is what the
            // MODEL reads, the ledger is what decides the client's "was not executed"
            // headline (see `DenialLedger`). The marker itself is ASCII with no backslash,
            // so it is never escaped and reaches the model verbatim.
            denials.record(tool_name);
            Err(anyhow::anyhow!(
                "{POLICY_DENIAL_PREFIX}{}",
                encoder_safe_reason(&reason)
            ))
        }
        ironhermes_hooks::GatedOutcome::Failed(reason) => {
            Err(anyhow::anyhow!("{}", encoder_safe_reason(&reason)))
        }
    }
}

/// Plan 05 task 3 (D-16, CLI-08 boundary contract): approval-gate a `write_file`/`patch`
/// call before it runs. Every workspace write is approval-gated (its diff is rendered
/// into the permission-request subject by `AcpApprovalGate::request_approval` — see
/// `approval_bridge.rs`); a write whose canonicalized target resolves OUTSIDE the session
/// cwd (`tool_render::is_outside_workspace` — traversal- and symlink-aware) is additionally
/// stated as such in the approval reason text, so the user sees exactly why they're being
/// asked. Fail-closed per plan 04's contract: any outcome other than an explicit
/// `ApprovalOutcome::Approved` means the write does NOT happen — the tool is never invoked
/// on that path, so a denial leaves the target file byte-identical to its pre-call state.
async fn gate_workspace_write(
    tool_name: &'static str,
    session_id: String,
    gate: Arc<dyn ironhermes_core::ApprovalGate>,
    cwd: std::path::PathBuf,
    tool: Option<Arc<dyn ironhermes_tools::registry::Tool>>,
    args: serde_json::Value,
    denials: DenialLedger,
) -> anyhow::Result<String> {
    let rel_path = args
        .get("path")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let target = cwd.join(&rel_path);
    let outside = crate::tool_render::is_outside_workspace(&cwd, &target);
    let reason = if outside {
        format!(
            "write path '{}' resolves OUTSIDE the session workspace root '{}' — treated as \
             a dangerous operation requiring explicit approval (D-16)",
            target.display(),
            cwd.display()
        )
    } else {
        format!(
            "workspace write to '{}' requires approval before it is applied (D-16)",
            target.display()
        )
    };

    match gate.request_approval(&session_id, tool_name, &reason, &args).await {
        ironhermes_core::ApprovalOutcome::Approved => match tool {
            Some(t) => t.execute(args).await,
            None => Err(anyhow::anyhow!(
                "'{tool_name}' tool not registered on this runtime"
            )),
        },
        _ => {
            denials.record(tool_name);
            // `reason` above is handed to the approval prompt UNescaped — the operator
            // should see the real path. Escaping happens only here, on the way into the
            // model-facing error text that the downstream wrapper JSON-encodes by hand.
            Err(anyhow::anyhow!(
                "{POLICY_DENIAL_PREFIX}{}",
                encoder_safe_reason(&format!(
                    "write to '{}' denied: {reason}",
                    target.display()
                ))
            ))
        }
    }
}

/// Fallback schema for `register_intercepted_or_replace("write_file", ...)` — used only if
/// `write_file` was never registered as a regular tool at all. Mirrors
/// `ironhermes_tools::file_tools::WriteFileTool::schema()` exactly so the LLM-facing tool
/// definition never drifts between the gated and ungated forms.
fn write_file_fallback_schema() -> ironhermes_core::ToolSchema {
    ironhermes_core::ToolSchema::new(
        "write_file",
        "Write content to a file, creating it or overwriting it if it already exists.",
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "Path to the file to write." },
                "content": { "type": "string", "description": "Content to write to the file." }
            },
            "required": ["path", "content"]
        }),
    )
}

/// Fallback schema for `register_intercepted_or_replace("patch", ...)` — mirrors
/// `ironhermes_tools::file_tools::PatchFileTool::schema()` exactly.
fn patch_fallback_schema() -> ironhermes_core::ToolSchema {
    ironhermes_core::ToolSchema::new(
        "patch",
        "Replace an exact string in a file with new content (first occurrence).",
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "Path to the file to patch." },
                "before": { "type": "string", "description": "The exact string to search for and replace." },
                "after": { "type": "string", "description": "The replacement string." }
            },
            "required": ["path", "before", "after"]
        }),
    )
}

/// Flatten the prompt's content blocks into a single string. Task 1's thin slice only
/// supports `ContentBlock::Text` (image/embedded-context land with COVERAGE.md's
/// `PromptCapabilities` in task 2); other block kinds are silently skipped rather than
/// erroring, since a client is free to send a block it advertised support for even before
/// the agent finishes negotiating a capability for it.
fn flatten_prompt_content(blocks: &[ContentBlock]) -> String {
    blocks
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text(text) => Some(text.text.clone()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Extract the human-readable name from any `McpServer` transport variant, for the D-17
/// log line only — never used to construct a connection or spawn a process.
fn mcp_server_name(server: &McpServer) -> &str {
    match server {
        McpServer::Http(s) => &s.name,
        McpServer::Sse(s) => &s.name,
        McpServer::Stdio(s) => &s.name,
        // `McpServer` is `#[non_exhaustive]` (e.g. the unstable ACP-transport variant,
        // feature-gated off in this crate) — fall back to a fixed label rather than
        // failing to compile against a future SDK variant.
        _ => "<unknown mcp server transport>",
    }
}

/// Map IronHermes's own [`HermesStopReason`] onto the ACP wire enum. `Natural` is the only
/// reason task 1's single-turn thin slice can realistically hit; the rest are mapped for
/// correctness so a turn that DOES hit budget/iteration/cancellation limits still returns a
/// well-formed response instead of panicking on a non-exhaustive match.
fn map_stop_reason(result: &AgentResult) -> AcpStopReason {
    match result.stop_reason {
        HermesStopReason::Natural => AcpStopReason::EndTurn,
        HermesStopReason::MaxIterations | HermesStopReason::BudgetExhausted => {
            AcpStopReason::MaxTurnRequests
        }
        HermesStopReason::Cancelled => AcpStopReason::Cancelled,
        HermesStopReason::DelegationFailures => AcpStopReason::Refusal,
    }
}

#[cfg(test)]
mod policy_denial_marker_tests {
    //! Plan 02 (D-12): `map_gated_outcome` must distinguish a policy denial ("not run")
    //! from a genuine tool failure ("ran and blew up") in the error TEXT it produces —
    //! both flow, unchanged, into the model-facing tool-result string and (via
    //! `event_bridge.rs`'s `tool_result_callback`) the client-facing `tool_call_update`.
    use super::*;

    #[test]
    fn denied_outcome_carries_the_policy_denial_prefix_and_the_original_reason() {
        let outcome = ironhermes_hooks::GatedOutcome::Denied("operator denied".to_string());
        let denials = DenialLedger::new();
        let err = map_gated_outcome("terminal", outcome, &denials)
            .expect_err("Denied must map to Err");
        let text = err.to_string();
        assert!(
            denials.take("terminal"),
            "a denial must ALSO be recorded structurally — the text is what the model \
             reads, the ledger is what decides the client's \"was not executed\" headline"
        );
        assert!(
            text.starts_with(POLICY_DENIAL_PREFIX),
            "denial error text must start with the policy-denial prefix: {text}"
        );
        assert!(
            text.contains("operator denied"),
            "denial error text must still carry the original reason: {text}"
        );
    }

    #[test]
    fn blocked_outcome_also_carries_the_policy_denial_prefix() {
        // A guardrail hard-block is a policy outcome, not a tool malfunction — it must be
        // just as distinguishable from an ordinary failure as an explicit denial.
        let outcome = ironhermes_hooks::GatedOutcome::Blocked("tier-2 hard block".to_string());
        let denials = DenialLedger::new();
        let err = map_gated_outcome("execute_code", outcome, &denials)
            .expect_err("Blocked must map to Err");
        let text = err.to_string();
        assert!(denials.take("execute_code"), "a hard block is a policy denial too");
        assert!(
            text.starts_with(POLICY_DENIAL_PREFIX),
            "blocked error text must start with the policy-denial prefix: {text}"
        );
        assert!(text.contains("tier-2 hard block"));
    }

    #[test]
    fn failed_outcome_does_not_carry_the_policy_denial_prefix() {
        // A tool that ran and failed must stay distinguishable from one that was never
        // run at all — mislabelling a real failure as a policy decision would be worse
        // than the ambiguity this plan is closing.
        let outcome = ironhermes_hooks::GatedOutcome::Failed("execution error: boom".to_string());
        let denials = DenialLedger::new();
        let err = map_gated_outcome("terminal", outcome, &denials)
            .expect_err("Failed must map to Err");
        let text = err.to_string();
        assert!(
            !denials.take("terminal"),
            "a tool that RAN and failed must not be recorded as never executed"
        );
        assert!(
            !text.starts_with(POLICY_DENIAL_PREFIX),
            "an ordinary tool failure must NOT be prefixed as a policy denial: {text}"
        );
        assert!(text.contains("boom"));
    }

    #[test]
    fn ran_outcome_is_unchanged_and_byte_identical() {
        let outcome = ironhermes_hooks::GatedOutcome::Ran("stdout: ok".to_string());
        let denials = DenialLedger::new();
        let output = map_gated_outcome("terminal", outcome, &denials).expect("Ran must map to Ok");
        assert_eq!(output, "stdout: ok");
        assert!(!denials.take("terminal"));
    }

    /// Reproduces `agent_loop`'s hand-built intercept-failure encoder verbatim (the
    /// `dispatch_intercepts` branch): the ONLY escaping it applies is `"` -> `'`. Any
    /// reason this crate produces has to be valid inside that string.
    fn wrap_like_agent_loop(err_text: &str) -> String {
        format!(
            r#"{{"error":"intercept_failed","reason":"{}"}}"#,
            err_text.replace('"', "'")
        )
    }

    fn reason_field(wrapped: &str) -> String {
        let parsed: serde_json::Value = serde_json::from_str(wrapped).unwrap_or_else(|e| {
            panic!("the model-facing tool result must be parseable JSON: {e}\n{wrapped}")
        });
        parsed["reason"]
            .as_str()
            .expect("reason must be a JSON string")
            .to_string()
    }

    #[test]
    fn a_windows_path_in_a_denial_reason_survives_the_hand_built_json_encoder() {
        // `\U` is not a valid JSON escape. Unescaped, this renders the model-facing tool
        // result unparseable while the operator's card still looks correct — the failure is
        // asymmetric, which is why it is easy to miss. The path must also arrive intact,
        // not slash-swapped: the model may need to reason about the real path.
        let reason = encoder_safe_reason(r"write to 'C:\Users\alice\notes.txt' denied");
        let decoded = reason_field(&wrap_like_agent_loop(&format!(
            "{POLICY_DENIAL_PREFIX}{reason}"
        )));
        assert!(decoded.starts_with(POLICY_DENIAL_PREFIX));
        assert!(
            decoded.contains(r"C:\Users\alice\notes.txt"),
            "the original path must survive the round trip: {decoded}"
        );
    }

    #[test]
    fn control_characters_in_a_reason_survive_the_hand_built_json_encoder() {
        // The model chooses the `path` argument and the command a guardrail quotes back, so
        // a literal newline or tab in the reason is reachable on every platform, not just
        // Windows.
        let raw = "write to '/tmp/a\nb\tc' denied: \u{7} bell";
        let decoded = reason_field(&wrap_like_agent_loop(&encoder_safe_reason(raw)));
        assert_eq!(
            decoded, raw,
            "escaping must round-trip to the original text exactly"
        );
    }

    #[test]
    fn a_denied_outcome_carrying_a_backslash_still_produces_parseable_json() {
        // End to end through the real gate site rather than the helper alone.
        let outcome = ironhermes_hooks::GatedOutcome::Denied(
            r"guardrail matched pattern \d+ in command".to_string(),
        );
        let denials = DenialLedger::new();
        let err = map_gated_outcome("terminal", outcome, &denials)
            .expect_err("Denied must map to Err");
        let decoded = reason_field(&wrap_like_agent_loop(&err.to_string()));
        assert!(decoded.starts_with(POLICY_DENIAL_PREFIX));
        assert!(decoded.contains(r"pattern \d+ in command"));
    }
}
