//! ACP permission bridge (D-14/D-15): `AcpApprovalGate` converts the agent runtime's
//! `ApprovalGate::request_approval` calls into ACP `session/request_permission` round
//! trips, fails closed on every non-approval outcome, and honours `allow_always` for the
//! CURRENT SESSION ONLY — never persisted to disk (D-14).
//!
//! # Fail-closed mapping
//!
//! The only path to `ApprovalOutcome::Approved` is an explicit `allow-once` or
//! `allow-always` selection. Every other resolution — `reject-once`, an unrecognized
//! option id, a `Cancelled` outcome, a client transport error, a timeout, or a client that
//! never advertised permission support — falls through to `ApprovalOutcome::Denied`. The
//! match arms below are written so denial is the WILDCARD arm, not an enumerated case, so
//! a future SDK revision that adds a new `RequestPermissionOutcome` variant defaults to
//! refusal rather than silent approval.
//!
//! # Session-scoped allow-always (D-14)
//!
//! `allow-always` calls `ApprovalsStore::approve_session` — the in-memory, never-persisted
//! session tier — and NOTHING else. This file must never call `ApprovalsStore::approve_always`
//! or `ApprovalsStore::save_to_disk`: doing so would let a grant made inside one editor
//! session outlive the process, which D-14 forbids.
//!
//! # Schema note (deviation from RESEARCH.md's illustrative JSON)
//!
//! The pinned `agent-client-protocol-schema` 1.5.0 `RequestPermissionRequest` has no
//! top-level `title`/`description` fields — RESEARCH.md's Code Examples JSON was a
//! simplified illustration. The real shape is `{ session_id, tool_call: ToolCallUpdate,
//! options }`; this module puts the human-readable title on
//! `ToolCallUpdateFields::title` and the description (tool name + reason + redacted
//! args) in a `ToolCallContent::Content` text block, verified directly against the
//! installed crate source this session.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use agent_client_protocol::schema::v1::{
    ContentBlock, PermissionOption, PermissionOptionKind, RequestPermissionOutcome,
    RequestPermissionRequest, RequestPermissionResponse, SessionId, TextContent, ToolCallContent,
    ToolCallId, ToolCallStatus, ToolCallUpdate, ToolCallUpdateFields,
};
use agent_client_protocol::{Client, ConnectionTo};
use async_trait::async_trait;
use ironhermes_core::{ApprovalGate, ApprovalOutcome, ApprovalsStore};

/// Default `session/request_permission` round-trip timeout — mirrors
/// `ApprovalsGatewayConfig::timeout_secs`'s own default (120s) so ACP waits as long as the
/// gateway's own approval coordinator does before failing closed.
pub const DEFAULT_APPROVAL_TIMEOUT: Duration = Duration::from_secs(120);

const ALLOW_ONCE_OPTION_ID: &str = "allow-once";
const ALLOW_ALWAYS_OPTION_ID: &str = "allow-always";
const REJECT_ONCE_OPTION_ID: &str = "reject-once";

/// Case-insensitive substrings that mark a JSON key as sensitive for the redaction pass
/// below — mirrors `ironhermes_core::audit`'s own (private) `DEFAULT_SENSITIVE_TOKENS`
/// list so the permission-prompt description applies the same secret-shaped-value floor
/// the audit log already does.
const SENSITIVE_KEY_TOKENS: &[&str] = &[
    "token",
    "api_key",
    "apikey",
    "secret",
    "password",
    "authorization",
    "bearer",
    "credential",
];

/// Hard cap (characters) on the serialized, redacted args string embedded in the
/// permission-prompt description — the request payload is rendered directly in the
/// editor UI, so this bounds both leak surface and prompt size.
const MAX_DESCRIPTION_ARGS_CHARS: usize = 500;

/// Anything that can carry a `session/request_permission` request to the client and
/// return its response. Abstracts a live `ConnectionTo<Client>` (production) from a
/// scripted fake (tests), mirroring `event_bridge::NotificationSink`'s split so this
/// module's translation/fail-closed logic never has to stand up a real connection to be
/// tested.
#[async_trait]
pub trait PermissionRequestSender: Send + Sync {
    async fn send_permission_request(
        &self,
        request: RequestPermissionRequest,
    ) -> Result<RequestPermissionResponse, agent_client_protocol::Error>;
}

/// Production sender: a real live connection. `send_request(..).block_task()` is the
/// SDK's "await this specific outgoing request's response" primitive — safe to call from
/// inside a `cx.spawn`-ed task (per the SDK's own doc example), which is exactly where
/// `handle_session_prompt` runs the whole turn, including this gate (plan 03's `cx.spawn`
/// restructuring is a load-bearing precondition for this working at all — see plan 03
/// SUMMARY's "Next Phase Readiness").
#[async_trait]
impl PermissionRequestSender for ConnectionTo<Client> {
    async fn send_permission_request(
        &self,
        request: RequestPermissionRequest,
    ) -> Result<RequestPermissionResponse, agent_client_protocol::Error> {
        self.send_request(request).block_task().await
    }
}

/// ACP's `ironhermes_core::ApprovalGate` implementation (D-14/D-15). One instance per
/// ACP session, bound at construction to that session's own `connection`, `session_id`,
/// and `approvals` store (RESEARCH Pitfall 5 — NEVER a process-wide shared store; sharing
/// one across sessions would let a grant in one editor window silently auto-approve the
/// same command in another).
pub struct AcpApprovalGate {
    connection: Arc<dyn PermissionRequestSender>,
    session_id: String,
    approvals: Arc<ApprovalsStore>,
    /// Whether the connected client can be asked at all. `false` short-circuits straight
    /// to `Denied` without sending anything — a client that cannot ask the user cannot
    /// authorise anything on the user's behalf. The pinned ACP schema advertises no
    /// explicit "permission capability" flag (unlike `terminal`/`fs`, which are real
    /// `ClientCapabilities` fields) since `session/request_permission` is a
    /// universally-expected client method, not an opt-in extension — production callers
    /// always construct this `true`; the `false` path exists for defensive fail-closed
    /// coverage and is exercised directly in tests.
    client_supports_permissions: bool,
    timeout: Duration,
    /// Plan 05 (D-16, T-36.8-25): the session's cwd. When set, a `write_file`/`patch`
    /// permission request's subject additionally carries the SAME rendered diff the
    /// `tool_call` update shows (`tool_render::render_pending_edit`) — so the user
    /// approves exactly what they can see, not just a text description of it. `None`
    /// (the default via `new`) keeps every pre-plan-05 caller/test compiling unchanged and
    /// falls back to the original text-only description.
    cwd: Option<PathBuf>,
}

impl AcpApprovalGate {
    pub fn new(
        connection: Arc<dyn PermissionRequestSender>,
        session_id: impl Into<String>,
        approvals: Arc<ApprovalsStore>,
        client_supports_permissions: bool,
        timeout: Duration,
    ) -> Self {
        Self {
            connection,
            session_id: session_id.into(),
            approvals,
            client_supports_permissions,
            timeout,
            cwd: None,
        }
    }

    /// Plan 05 (D-16): bind this gate to the session's cwd so `write_file`/`patch`
    /// permission requests carry the rendered diff. Builder method (not a `new(...)`
    /// parameter) so every existing call site — production and the ~15 test call sites in
    /// `tests/approval_bridge.rs`/`tests/execute_code_gate.rs` — keeps compiling unchanged.
    pub fn with_cwd(mut self, cwd: impl Into<PathBuf>) -> Self {
        self.cwd = Some(cwd.into());
        self
    }
}

#[async_trait]
impl ApprovalGate for AcpApprovalGate {
    async fn request_approval(
        &self,
        _session_id: &str,
        tool_name: &str,
        reason: &str,
        args: &serde_json::Value,
    ) -> ApprovalOutcome {
        let cache_key = build_cache_key(tool_name, args);

        // D-14: the session tier is the ONLY tier this gate ever consults or writes.
        if self.approvals.is_session_approved(&cache_key).await {
            return ApprovalOutcome::Approved;
        }

        if !self.client_supports_permissions {
            return ApprovalOutcome::Denied;
        }

        let tool_call_id = ToolCallId::new(format!("approval-{}", uuid::Uuid::new_v4()));
        let description = build_description(tool_name, reason, args);
        let mut content = vec![ToolCallContent::from(ContentBlock::Text(TextContent::new(
            description,
        )))];
        // Plan 05 (D-16, T-36.8-25): a write_file/patch permission request ALSO carries
        // the rendered diff — the SAME function `render_call_content` uses for the
        // `tool_call` update, so the user approves exactly the change that will be
        // applied, not just a textual description of it.
        if let (true, Some(cwd)) = (
            tool_name == "write_file" || tool_name == "patch",
            self.cwd.as_deref(),
        ) {
            content.push(crate::tool_render::render_pending_edit(tool_name, args, cwd));
        }
        let tool_call_update = ToolCallUpdate::new(
            tool_call_id,
            ToolCallUpdateFields::new()
                .title(permission_title(tool_name))
                .status(ToolCallStatus::Pending)
                .content(content),
        );

        let options = offered_permission_options();

        let request = RequestPermissionRequest::new(
            SessionId::new(self.session_id.clone()),
            tool_call_update,
            options,
        );

        let response = tokio::time::timeout(
            self.timeout,
            self.connection.send_permission_request(request),
        )
        .await;

        // Denial is the WILDCARD arm at every level of this match — see module doc.
        match response {
            Ok(Ok(response)) => match response.outcome {
                RequestPermissionOutcome::Selected(selected) => {
                    match selected.option_id.0.as_ref() {
                        ALLOW_ONCE_OPTION_ID => ApprovalOutcome::Approved,
                        ALLOW_ALWAYS_OPTION_ID => {
                            self.approvals.approve_session(&cache_key).await;
                            ApprovalOutcome::Approved
                        }
                        _ => ApprovalOutcome::Denied,
                    }
                }
                // Covers `Cancelled` and any future non-exhaustive variant.
                _ => ApprovalOutcome::Denied,
            },
            // Transport error (peer closed, malformed response, etc.) — fail closed.
            Ok(Err(_transport_error)) => ApprovalOutcome::Denied,
            // Configured timeout elapsed before the client answered — fail closed. The
            // dropped `SentRequest` (inside `block_task`'s future, dropped when this
            // `tokio::time::timeout` future is cancelled) auto-sends a `$/cancel_request`
            // to the peer per the SDK's own `Drop` behavior.
            Err(_elapsed) => ApprovalOutcome::Denied,
        }
    }

    async fn resolve(&self, _session_id: &str, _approved: bool) -> bool {
        // ACP has no out-of-band resolution channel — the client answers the exact
        // request it was sent inline in `request_approval`. The trait's contract already
        // treats `false` as "no pending approval to resolve here."
        tracing::debug!(
            "AcpApprovalGate::resolve called — ACP has no out-of-band resolution channel"
        );
        false
    }
}

fn permission_title(tool_name: &str) -> String {
    format!("Allow {tool_name}?")
}

/// The `PermissionOption` array `AcpApprovalGate` offers on every `session/request_permission`
/// call — extracted to a `pub` accessor (Phase 47.7 plan 01 task 2) so an integration test can
/// assert on the actual serialized JSON a future refactor might drift, rather than duplicating
/// the literal (a duplicated literal cannot detect drift). buzz-acp's own permission matcher
/// depends on finding an option whose `kind` is the literal snake_case string `reject_once`
/// (RESEARCH Pattern 3, `~/code/buzz/crates/buzz-acp/src/acp.rs:2021-2048`, read-only
/// reference) — this is the exact array that must keep offering one.
///
/// A5 (RESEARCH assumption log): the schema also has a `reject-always` kind; this collapses
/// reject to the single `reject-once` option here because D-14 forbids any persistent grant,
/// and a persistent *denial* would be an equally unwanted disk-backed state.
#[must_use]
pub fn offered_permission_options() -> Vec<PermissionOption> {
    vec![
        PermissionOption::new(
            ALLOW_ONCE_OPTION_ID,
            "Allow once",
            PermissionOptionKind::AllowOnce,
        ),
        PermissionOption::new(
            ALLOW_ALWAYS_OPTION_ID,
            "Allow always",
            PermissionOptionKind::AllowAlways,
        ),
        PermissionOption::new(
            REJECT_ONCE_OPTION_ID,
            "Reject",
            PermissionOptionKind::RejectOnce,
        ),
    ]
}

/// Build the session-scoped cache key: tool name + the normalized command/argument text,
/// so distinct commands within the same tool are distinct approval classes (RESEARCH
/// Pitfall 5's "keyed by normalized command text, not session id" scope, task 2).
fn build_cache_key(tool_name: &str, args: &serde_json::Value) -> String {
    let material = args
        .get("command")
        .and_then(|v| v.as_str())
        .map(str::to_owned)
        .unwrap_or_else(|| args.to_string());
    format!("{tool_name}:{}", ApprovalsStore::normalize_command(&material))
}

/// Combine the tool name and reason into the permission prompt's description, with
/// redacted + bounded-length arguments appended. Tool arguments can carry credentials —
/// the rendered request goes straight into the editor UI, so this is the only line of
/// defense against forwarding a raw secret into that prompt.
fn build_description(tool_name: &str, reason: &str, args: &serde_json::Value) -> String {
    format!(
        "IronHermes wants to run `{tool_name}`: {reason}\nArguments: {}",
        redact_and_truncate_args(args)
    )
}

fn redact_and_truncate_args(args: &serde_json::Value) -> String {
    let redacted = redact_json(args);
    let serialized = serde_json::to_string(&redacted).unwrap_or_else(|_| "{}".to_string());
    truncate_chars(&serialized, MAX_DESCRIPTION_ARGS_CHARS)
}

fn redact_json(value: &serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Object(map) => {
            let mut out = serde_json::Map::with_capacity(map.len());
            for (k, v) in map {
                if is_sensitive_key(k) {
                    out.insert(k.clone(), serde_json::Value::String("[REDACTED]".to_string()));
                } else {
                    out.insert(k.clone(), redact_json(v));
                }
            }
            serde_json::Value::Object(out)
        }
        serde_json::Value::Array(items) => {
            serde_json::Value::Array(items.iter().map(redact_json).collect())
        }
        other => other.clone(),
    }
}

fn is_sensitive_key(key: &str) -> bool {
    let lower = key.to_lowercase();
    SENSITIVE_KEY_TOKENS.iter().any(|tok| lower.contains(tok))
}

fn truncate_chars(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        return s.to_string();
    }
    let truncated: String = s.chars().take(max_chars).collect();
    format!("{truncated}…(truncated)")
}
