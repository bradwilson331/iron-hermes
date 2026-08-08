//! Async approval engine — `ApprovalCoordinator` + `PendingApproval` + `GatewayApprovalGate`.
//!
//! Enforces Phase 45 decisions end-to-end, entirely in-process:
//!
//! - **D-02:** one active pending approval per session (second request → `AlreadyPending`).
//! - **D-03 (positive):** CLI-set `session`/`always` approvals bypass the chat prompt; the
//!   coordinator reads `ApprovalsStore` but NEVER writes to it (D-03 negative).
//! - **D-04:** timeout read from `ApprovalsGatewayConfig.timeout_secs` (default 120 s).
//! - **D-05:** on timeout the coordinator posts an expiry notice to the originating chat via
//!   `PlatformAdapter`; no silent expiry.
//! - **D-06:** when `tool_name` contains glob/variable metacharacters (`*`, `?`, `$`, `~`)
//!   the approval prompt appends a literal TOCTOU warning (human-judgment layer, not a sandbox).
//! - **D-07 (positive):** `PendingApproval` lives only in a `tokio::sync::Mutex<HashMap>`;
//!   a dropped `oneshot::Sender` (restart/crash) makes the `Receiver` resolve to
//!   `Err(RecvError)` → `Denied` — fail-closed by construction.
//! - **D-07 (negative):** `PendingApproval` holds a non-serializable `oneshot::Sender`; no
//!   code path writes it to `~/.ironhermes/` (D-12 no audit.jsonl either).
//! - **T-45-10:** `tool_name` is newline-stripped and truncated to ~200 chars before prompt
//!   interpolation (prompt-injection mitigation).

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use ironhermes_core::{ApprovalGate, ApprovalOutcome, ApprovalsStore, AuditLog, KeyKind};

use crate::adapter::PlatformAdapter;
use crate::rate_limiter::with_rate_limit_retry;

// ─────────────────────────────────────────────────────────────────────────────
// PendingApproval
// ─────────────────────────────────────────────────────────────────────────────

/// A parked approval request for one session.
///
/// Lives exclusively in the `ApprovalCoordinator.pending` map (in-memory only — D-07).
/// The `responder` field is intentionally non-serializable; `PendingApproval` can
/// never be persisted to disk by construction.
// Fields tool_name/reason/chat_id are stored for future introspection
// (introspection hooks land in a later phase — D-07 in-memory only for Phase 45).
#[allow(dead_code)]
pub struct PendingApproval {
    tool_name: String,
    reason: String,
    /// Wall-clock deadline (used for opportunistic zombie cleanup in `resolve()`).
    expires_at: Instant,
    /// Oneshot sender.  Dropping it causes the parked `rx.await` to resolve to
    /// `Err(RecvError)`, which the coordinator maps to `Denied` (D-07 fail-closed).
    responder: tokio::sync::oneshot::Sender<bool>,
    /// Originating chat — needed to deliver the D-05 expiry notice without a
    /// reverse session_id→chat_id lookup.
    chat_id: String,
}

// ─────────────────────────────────────────────────────────────────────────────
// ApprovalCoordinator
// ─────────────────────────────────────────────────────────────────────────────

/// In-memory, fail-closed approval coordinator.
///
/// Shared across turns as `Arc<ApprovalCoordinator>`.  Each incoming turn constructs
/// a `GatewayApprovalGate` that wraps an `Arc` clone and binds the turn's `chat_id`.
///
/// Phase 47.6 Plan 06 (P1-2): the gateway constructs ONE instance of this struct PER
/// PLATFORM (Telegram gets its own, Buzz gets its own) rather than a single shared
/// instance. `adapter` below is a single field bound once at construction and held
/// for this coordinator's entire lifetime — there is no way to "add a Buzz case" to
/// an existing instance, so a second platform that wants `/approve`/`/deny` support
/// gets a second coordinator instance instead. `runner.rs`'s
/// `build_platform_approval_coordinator` is the one call site every platform's
/// coordinator is built through, so the shared `ApprovalsStore`/`AuditLog` Arcs stay
/// provably the same objects across platforms. Session keys already carry the
/// originating platform (they are the canonical per-turn session id, unique per
/// platform's own session-key derivation), so two coordinators' `pending` maps can
/// never collide on the same key even though each is a bare `HashMap<String, _>`
/// with no platform discriminator of its own — pinned by
/// `two_coordinators_have_independent_pending_maps`.
pub struct ApprovalCoordinator {
    /// MUST be `tokio::sync::Mutex` — the guard is held across `.await` points inside
    /// `request()` (prompt send + oneshot wait).  Using `std::sync::Mutex` would
    /// make the future non-`Send` and break the spawned-task executor contract.
    pending: tokio::sync::Mutex<HashMap<String, PendingApproval>>,
    timeout_secs: u64,
    adapter: Arc<dyn PlatformAdapter>,
    /// Read-only reference to the Phase 42 CLI approval store (D-03 positive).
    /// The coordinator NEVER calls `approve_session` / `approve_always` / `save_to_disk`
    /// on this store (D-03 negative, D-07 negative).
    approvals_store: Arc<ApprovalsStore>,
    /// Phase 46 D-01/D-02: append-only mutation audit log. Every resolution return
    /// point (bypass / operator-approved / operator-denied / dropped-sender / timeout)
    /// appends exactly one entry before returning to the caller.
    audit_log: Arc<AuditLog>,
}

impl ApprovalCoordinator {
    /// Construct a coordinator.
    ///
    /// `timeout_secs` should come from `config.approvals.timeout_secs` (default 120).
    pub fn new(
        timeout_secs: u64,
        adapter: Arc<dyn PlatformAdapter>,
        approvals_store: Arc<ApprovalsStore>,
        audit_log: Arc<AuditLog>,
    ) -> Self {
        Self {
            pending: tokio::sync::Mutex::new(HashMap::new()),
            timeout_secs,
            adapter,
            approvals_store,
            audit_log,
        }
    }

    /// Derive the audit `kind` + `server` fields from `tool_name` (Phase 46 D-03):
    /// `"mcp_mutation"` + the segment before `__` when present, else `"shell_command"`.
    fn derive_kind_and_server(tool_name: &str) -> (&'static str, Option<String>) {
        match tool_name.split_once("__") {
            Some((server, _)) => ("mcp_mutation", Some(server.to_string())),
            None => ("shell_command", None),
        }
    }

    /// Build an [`ironhermes_core::AuditEntry`] for this coordinator's known chat
    /// context (surface derived from the bound `PlatformAdapter`, e.g. `"telegram"`).
    #[allow(clippy::too_many_arguments)]
    fn build_audit_entry(
        &self,
        session_id: &str,
        chat_id: &str,
        tool_name: &str,
        reason: &str,
        args: &serde_json::Value,
        decision: &str,
        resolution: &str,
    ) -> ironhermes_core::AuditEntry {
        let (kind, server) = Self::derive_kind_and_server(tool_name);
        self.audit_log.make_entry(
            kind,
            tool_name,
            server,
            session_id,
            self.adapter.platform().to_string(),
            chat_id,
            reason,
            decision,
            resolution,
            args,
        )
    }

    /// D-02/CFL-03: on the denied/timeout paths there is no pending operation to
    /// block, so an audit append failure cannot fail-closed the way the
    /// approved/bypass paths do. Instead it must be operator-visible — a `warn!`
    /// plus a best-effort chat notice — never a silent drop.
    async fn notify_audit_failure(&self, chat_id: &str, tool_name: &str, context: &str) {
        tracing::warn!(
            tool_name,
            context,
            "audit log append failed — CFL-03 entry may be missing; outcome unchanged \
             (operator-visible, not fail-closed, on this non-executing path)"
        );
        let notice = format!(
            "\u{26a0}\u{fe0f} Audit log write failed while recording this {context} action \
             (`{tool_name}`). The outcome above still stands, but this event may not be \
             durably recorded."
        );
        let _ = with_rate_limit_retry(|| self.adapter.send_message(chat_id, &notice, None)).await;
    }

    /// Park the calling task until the operator responds or the timeout elapses.
    ///
    /// Decision order:
    /// 1. **D-03:** return `Approved` immediately if `ApprovalsStore` already carries a
    ///    `session` or `always` approval for the normalised `tool_name`.
    /// 2. **D-02:** return `AlreadyPending` if the session already has a pending request.
    /// 3. Insert a `PendingApproval`, send the approval prompt, then await the oneshot.
    /// 4. **D-04/D-05:** on timeout send an expiry notice and return `TimedOut`.
    pub async fn request(
        &self,
        session_id: &str,
        chat_id: &str,
        tool_name: &str,
        reason: &str,
        args: &serde_json::Value,
    ) -> ApprovalOutcome {
        // ── D-03: honor CLI-set approvals without prompting chat ──────────────
        let cmd_key = ApprovalsStore::normalize_command(tool_name);
        if self.approvals_store.is_session_approved(&cmd_key).await
            || self
                .approvals_store
                .is_always_approved(&cmd_key, KeyKind::Command)
                .await
        {
            let entry = self.build_audit_entry(
                session_id, chat_id, tool_name, reason, args, "approved", "bypass",
            );
            if self.audit_log.append(&entry).await.is_err() {
                // D-02: fail-closed — a broken audit sink must block even a
                // bypass-approved mutation from running unrecorded.
                tracing::warn!(
                    tool_name,
                    "audit log append failed for bypass-approved resolution; \
                     downgrading to Denied (D-02 fail-closed)"
                );
                return ApprovalOutcome::Denied;
            }
            return ApprovalOutcome::Approved;
        }

        // ── D-02: one active pending per session ──────────────────────────────
        let (tx, rx) = tokio::sync::oneshot::channel::<bool>();
        {
            let mut map = self.pending.lock().await;
            let now = Instant::now();
            // HI-01 fix: purge expired/zombie entries on the INSERT path too.
            // Only resolve() purged before, but resolve() is not guaranteed to run —
            // a dropped request() future (turn cancelled at rx.await) never reaches
            // its own timeout-cleanup block, leaving a PendingApproval (with a live
            // future expires_at) in the map. Without this purge a subsequent
            // request() for that session returns AlreadyPending forever, wedging the
            // session until some unrelated resolve() happens to run retain().
            map.retain(|k, pa| k.as_str() == session_id || pa.expires_at > now);
            // Evict this session's OWN zombie (present but already expired) before the
            // D-02 gate, so a stale entry can never masquerade as an active pending.
            if let Some(pa) = map.get(session_id) {
                if pa.expires_at > now {
                    return ApprovalOutcome::AlreadyPending;
                }
                map.remove(session_id);
            }
            map.insert(
                session_id.to_owned(),
                PendingApproval {
                    tool_name: tool_name.to_owned(),
                    reason: reason.to_owned(),
                    expires_at: now + Duration::from_secs(self.timeout_secs),
                    responder: tx,
                    chat_id: chat_id.to_owned(),
                },
            );
        } // lock released — resolve() can now acquire it

        // ── T-45-10: sanitise tool_name before prompt interpolation ────────────
        // Strip newlines + CR, truncate to 200 chars to limit prompt-injection surface.
        let safe_tool: String = tool_name
            .replace(['\n', '\r'], " ")
            .chars()
            .take(200)
            .collect();

        // ── F4 / LO-01: sanitise `reason` too ─────────────────────────────────
        // The MCP reason embeds the RAW remote-server-supplied tool_name
        // (guardrail.rs McpMutationGuardrail: "MCP tool '<tool_name>' contains a
        // destructive verb ..."), which bypasses the newline-strip+truncate applied
        // to `safe_tool`. Remote MCP tool names are attacker-influenceable (D-09), so
        // apply the SAME treatment to `reason` before interpolation to prevent
        // prompt-injection (fake descriptions / line breaks hiding the real tool).
        let safe_reason: String = reason
            .replace(['\n', '\r'], " ")
            .chars()
            .take(200)
            .collect();

        // ── D-06: TOCTOU warning for shell-expansion metacharacters ───────────
        let toctou = if has_shell_expansion(tool_name) {
            "\n\u{26a0}\u{fe0f} Warning: command contains globs/variables \u{2014} \
             shell expansion is NOT previewed at approval time (TOCTOU risk)."
        } else {
            ""
        };

        let prompt = format!(
            "\u{1f6d1} Approval required\n\
             Tool: `{safe_tool}`\n\
             Reason: {safe_reason}{toctou}\n\
             Reply `/approve` to allow or `/deny` to cancel \
             (auto-denies in {}s).",
            self.timeout_secs
        );

        // Send the prompt — mirror handler.rs line 835.
        let _ = with_rate_limit_retry(|| self.adapter.send_message(chat_id, &prompt, None)).await;

        // ── D-04: await with configured timeout ───────────────────────────────
        match tokio::time::timeout(Duration::from_secs(self.timeout_secs), rx).await {
            Ok(Ok(true)) => {
                let entry = self.build_audit_entry(
                    session_id, chat_id, tool_name, reason, args, "approved", "operator",
                );
                if self.audit_log.append(&entry).await.is_err() {
                    // D-02: fail-closed — a broken audit sink must block even an
                    // operator-approved mutation from running unrecorded.
                    tracing::warn!(
                        tool_name,
                        "audit log append failed for operator-approved resolution; \
                         downgrading to Denied (D-02 fail-closed)"
                    );
                    return ApprovalOutcome::Denied;
                }
                ApprovalOutcome::Approved
            }
            // Ok(false) = operator explicitly denied.
            Ok(Ok(false)) => {
                let entry = self.build_audit_entry(
                    session_id, chat_id, tool_name, reason, args, "denied", "operator",
                );
                if self.audit_log.append(&entry).await.is_err() {
                    // No op executes on this path (D-02 is execution-scoped), so a
                    // broken sink is operator-visible rather than fail-closed — CFL-03
                    // completeness without blocking a non-executing op.
                    self.notify_audit_failure(chat_id, tool_name, "denied")
                        .await;
                }
                ApprovalOutcome::Denied
            }
            // Ok(Err(_)) = oneshot sender dropped (e.g. simulated restart) → fail-closed (D-07)
            Ok(Err(_)) => {
                let entry = self.build_audit_entry(
                    session_id, chat_id, tool_name, reason, args, "denied", "dropped",
                );
                if self.audit_log.append(&entry).await.is_err() {
                    self.notify_audit_failure(chat_id, tool_name, "denied")
                        .await;
                }
                ApprovalOutcome::Denied
            }
            Err(_elapsed) => {
                // MED-02 fix: evict the entry FIRST, BEFORE awaiting the expiry
                // send_message. Previously the await happened first, leaving the
                // entry in the map during the send window — a racing /approve could
                // remove()+send() and report "Approved — running command" while this
                // request() had already committed to TimedOut ("did not execute"),
                // producing contradictory operator feedback. Evicting first means a
                // racing resolve() correctly finds no entry and returns false.
                self.pending.lock().await.remove(session_id);

                // Phase 46 D-01/D-02: append the timed_out entry BEFORE building the
                // expiry notice so a sink failure can be folded into the SAME chat
                // message rather than sending a second one.
                let entry = self.build_audit_entry(
                    session_id,
                    chat_id,
                    tool_name,
                    reason,
                    args,
                    "timed_out",
                    "timeout",
                );
                let audit_failed = self.audit_log.append(&entry).await.is_err();
                if audit_failed {
                    tracing::warn!(
                        tool_name,
                        "audit log append failed for timed_out resolution; CFL-03 entry \
                         may be missing (operator-visible, not fail-closed, on this \
                         non-executing path)"
                    );
                }

                // ── D-05: notify originating chat on expiry ───────────────────
                let mut expiry = format!(
                    "\u{23f0} Approval request expired \u{2014} `{safe_tool}` was NOT executed \
                     (no response within {}s).",
                    self.timeout_secs
                );
                if audit_failed {
                    expiry.push_str(
                        "\n\u{26a0}\u{fe0f} Audit log write failed for this event \u{2014} \
                         it may not be durably recorded (CFL-03).",
                    );
                }
                let _ = with_rate_limit_retry(|| self.adapter.send_message(chat_id, &expiry, None))
                    .await;
                ApprovalOutcome::TimedOut
            }
        }
    }

    /// Resolve a pending approval, unblocking the parked `request()` task.
    ///
    /// - `approved: true` → sends `true` on the oneshot → `request()` returns `Approved`.
    /// - `approved: false` → sends `false` on the oneshot → `request()` returns `Denied`.
    ///
    /// Returns `false` if no pending approval exists for `session_id` (idempotent —
    /// safe to call on `/deny` with no pending request).
    ///
    /// Also opportunistically purges zombie entries whose `expires_at` is in the past
    /// (cheap O(n) cleanup — the map is bounded to one entry per active session).
    pub async fn resolve(&self, session_id: &str, approved: bool) -> bool {
        let mut map = self.pending.lock().await;
        // Purge entries that have already timed out but weren't cleaned up
        // (e.g. the request() future was dropped before the cleanup block ran).
        let now = Instant::now();
        map.retain(|k, pa| k.as_str() == session_id || pa.expires_at > now);
        // Resolve the specific session.
        if let Some(pa) = map.remove(session_id) {
            // MED-02 fix: report success ONLY when the oneshot send actually reaches
            // a LIVE receiver. A zombie entry whose request() future was dropped (or
            // already returned TimedOut/Denied) has a dead receiver — send() returns
            // Err — and must NOT report "Approved" to the operator. Returning
            // send().is_ok() makes /approve on a dead entry fall through to the
            // "No pending approval for this session." reply (fail-closed, no false
            // positive), instead of the previous unconditional `true`.
            pa.responder.send(approved).is_ok()
        } else {
            false
        }
    }

    /// CR-01 fix (47.6 code review): resolve a pending approval by the
    /// **stable DM-reachable identity** the approval prompt was addressed
    /// to, rather than by session id.
    ///
    /// # Why this exists
    ///
    /// For a Buzz **channel**-originated turn, `approval_target_for`
    /// (handler.rs) addresses the prompt to `event.sender_id` — the
    /// author's Nostr pubkey, HEX-encoded — and that same hex string is
    /// what gets stored as this `PendingApproval`'s `chat_id`
    /// (`GatewayApprovalGate::new` binds `approval_target` verbatim as
    /// `chat_id`, and `ApprovalCoordinator::request` stores the `chat_id`
    /// parameter it's given unchanged). But `request()`'s `session_id`
    /// parameter for that same turn is derived from the **channel's**
    /// `SessionKey` (handler.rs `run_agent`), not the operator's identity.
    /// When the operator later replies to the DM prompt with `/approve`,
    /// `run_buzz_adapter`'s DM-receive path builds a fresh `MessageEvent`
    /// whose `chat_id` is the operator's own **npub** (bech32) and whose
    /// `sender_id` is the operator's pubkey, again HEX-encoded
    /// (`buzz.rs`'s DM-receive construction: `chat_id: sender_npub,
    /// sender_id: sender_hex`). `handle_slash_command` derives its
    /// `ctx_session_id` from THIS event's own `SessionKey`
    /// (`Buzz:<npub>:<hex>`), which never matches the channel turn's
    /// `SessionKey` (`Buzz:<channel-id>:<hex>`) — so `resolve()` always
    /// misses for a channel-originated approval.
    ///
    /// The fix: since the channel turn's `PendingApproval.chat_id` and the
    /// DM reply's `event.sender_id` are BOTH the operator's pubkey in HEX
    /// form, they compare equal directly — no npub/hex normalization is
    /// needed for this specific path (verified by reading both construction
    /// sites above; do not assume this holds for a hypothetical channel
    /// `chat_id` shaped differently — it holds because `approval_target_for`
    /// specifically returns `event.sender_id`, which is always hex).
    ///
    /// # D-02 tie-break
    ///
    /// The same operator can have two channel-turn approvals pending
    /// concurrently (e.g. two different channels, or two dangerous commands
    /// in quick succession) — both would DM-prompt the SAME `chat_id`
    /// (their own pubkey), so more than one pending entry can match. Pick
    /// deterministically: the entry with the EARLIEST `expires_at` — since
    /// every entry's `expires_at` is `insert_time + timeout_secs` with a
    /// fixed `timeout_secs`, earliest `expires_at` is equivalent to
    /// oldest-inserted. Resolving oldest-first means a rapid double
    /// `/approve` from the operator drains the queue in the order the
    /// commands were actually issued, matching intuitive expectations.
    ///
    /// Purges expired entries first (mirrors `resolve()`'s opportunistic
    /// zombie cleanup) so a stale entry never masquerades as a live match.
    pub async fn resolve_by_chat_id(&self, chat_id: &str, approved: bool) -> bool {
        let mut map = self.pending.lock().await;
        let now = Instant::now();
        map.retain(|_, pa| pa.expires_at > now);
        let oldest_match = map
            .iter()
            .filter(|(_, pa)| pa.chat_id == chat_id)
            .min_by_key(|(_, pa)| pa.expires_at)
            .map(|(session_id, _)| session_id.clone());
        let Some(session_id) = oldest_match else {
            return false;
        };
        if let Some(pa) = map.remove(&session_id) {
            // MED-02 fix (see resolve()'s identical rationale): only report
            // success when the oneshot send actually reaches a LIVE receiver.
            pa.responder.send(approved).is_ok()
        } else {
            false
        }
    }

    /// Test-only helper: clear all pending entries, dropping every oneshot sender.
    ///
    /// This simulates a coordinator restart with outstanding pending approvals —
    /// every parked `request()` future resolves its `rx` to `Err(RecvError)` → `Denied`
    /// (D-07 fail-closed).  Compiled only in `#[cfg(test)]`.
    #[cfg(test)]
    pub(crate) async fn simulate_restart(&self) {
        self.pending.lock().await.clear();
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// GatewayApprovalGate
// ─────────────────────────────────────────────────────────────────────────────

/// Per-turn `ApprovalGate` that binds a specific `chat_id` to the shared coordinator.
///
/// Plan 04 constructs one `GatewayApprovalGate` per incoming turn and threads it into
/// `AgentLoop::with_approval_gate`.  The `chat_id` is bound at construction time
/// (mirrors how `handler.rs` binds `session_key` per turn), so `request_approval`
/// never needs a session_id→chat_id reverse lookup.
pub struct GatewayApprovalGate {
    coordinator: Arc<ApprovalCoordinator>,
    chat_id: String,
}

impl GatewayApprovalGate {
    /// Bind a coordinator + turn-specific `chat_id`.
    pub fn new(coordinator: Arc<ApprovalCoordinator>, chat_id: String) -> Self {
        Self {
            coordinator,
            chat_id,
        }
    }
}

#[async_trait]
impl ApprovalGate for GatewayApprovalGate {
    async fn request_approval(
        &self,
        session_id: &str,
        tool_name: &str,
        reason: &str,
        args: &serde_json::Value,
    ) -> ApprovalOutcome {
        self.coordinator
            .request(session_id, &self.chat_id, tool_name, reason, args)
            .await
    }

    async fn resolve(&self, session_id: &str, approved: bool) -> bool {
        self.coordinator.resolve(session_id, approved).await
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Returns `true` when `s` contains shell-expansion metacharacters
/// (`*`, `?`, `$`, `~`) that could differ at expansion time (D-06 / T-45-09).
pub fn has_shell_expansion(s: &str) -> bool {
    s.contains(['*', '?', '$', '~'])
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod approval_tests;
