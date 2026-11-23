//! `WebApprovalGate`: the `ironhermes_core::ApprovalGate` for the web/desktop surface.
//!
//! # Two postures
//!
//! - **Interactive** ([`WebApprovalGate::interactive`], Phase 57 D-14, folding backlog
//!   999.3). A gated command that needs approval is *parked*: the gate registers a
//!   pending approval in the shared [`WebApprovalRegistry`], tells the browser through
//!   the turn's event sink ([`WebGateEvent::ApprovalRequested`]) and waits. The browser
//!   answers through the authenticated `resolve_web_approval(approval_id, approved)`
//!   server function (`approval_api.rs`), which sits behind `require_auth` like every
//!   server function. Nothing typed into the chat, and nothing a model emits, can reach
//!   the registry: only that function, with the matching unexpired `approval_id`.
//! - **Fail-closed** ([`WebApprovalGate::new`] / [`WebApprovalGate::with_notice_sink`]).
//!   No registry, so there is nobody to ask: `request_approval` denies at once. This is
//!   what a caller with no event channel gets (an approval nobody can see would only
//!   hang until the timeout) and what the pre-Phase-57 tests exercise.
//!
//! # Terminal states: everything but an in-time Approve is a denial
//!
//! | Situation                                         | Outcome                 | `outcome` word |
//! |---------------------------------------------------|-------------------------|----------------|
//! | operator approves before the deadline             | `Approved`              | `approved`     |
//! | operator denies                                   | `Denied`                | `denied`       |
//! | `approvals.timeout_secs` elapses                  | `TimedOut`              | `timed_out`    |
//! | the turn is cancelled / the sender is dropped     | `Denied`                | `cancelled`    |
//! | a second request while one is parked (per session)| `AlreadyPending`        | (no event)     |
//! | unknown, expired, or other-session `approval_id`  | resolve returns `false` | (changes nothing) |
//!
//! Each of the first four emits one [`WebGateEvent::ApprovalResolved`] carrying the same
//! tool name the request named. `execute_gated_command` audits the resolution exactly
//! once (the `shell_command` row), so this module writes no audit row of its own.
//!
//! The pending map is a `std::sync::Mutex` that is never held across an `.await`; every
//! read is a plain lock read, which matters because the web server's per-connection
//! `LocalSet` forbids `block_in_place`.
//!
//! # What the browser is told
//!
//! The reason is redacted (a `password=...` style value becomes `[REDACTED]`), stripped
//! of control characters and truncated to 300 characters. The raw `args` the gate
//! receives are never forwarded anywhere: the event carries a tool name and a reason.
//!
//! Tier-2 (`DangerousCommandGuardrail::Block`) commands and every other Block are
//! decided before this gate is consulted; this gate changes nothing about that floor.
//!
//! Cites: D-14 (web approval path), D-16 (security events), D-08 (forced approval on
//! remote), 999.3 SC-1..SC-5.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use ironhermes_core::{ApprovalGate, ApprovalOutcome, SecurityEvent, YoloBypassSummary};
use tokio::sync::oneshot;

/// Maximum characters of the operator-facing approval reason sent to the browser.
const APPROVAL_REASON_MAX_CHARS: usize = 300;

/// Case-insensitive substrings that mark a `key=value` word in a reason as a secret.
/// Mirrors the audit log's default sensitive-key list (`ironhermes_core::audit`); that
/// list is private to core and keyed on JSON object keys, so free text needs its own pass.
const SENSITIVE_WORD_TOKENS: &[&str] = &[
    "token",
    "api_key",
    "apikey",
    "secret",
    "password",
    "authorization",
    "bearer",
    "credential",
];

const REDACTED: &str = "[REDACTED]";

/// One thing a web gate tells the surface about. `ws.rs` turns each into the matching
/// `ChatStreamEvent` on the turn's own event channel.
#[derive(Debug, Clone)]
pub enum WebGateEvent {
    /// A turn's aggregated yolo/bypass summary (Phase 54.3).
    YoloBypass(YoloBypassSummary),
    /// A command is parked awaiting the operator (D-14).
    ApprovalRequested {
        session_id: String,
        approval_id: String,
        tool: String,
        reason: String,
    },
    /// The parked approval ended. `outcome` is `approved`, `denied`, `timed_out` or
    /// `cancelled`; `tool` is the tool name the request named (never arguments).
    ApprovalResolved {
        session_id: String,
        approval_id: String,
        tool: String,
        outcome: String,
    },
    /// A typed security event (D-16); plan 14 renders it.
    Security(SecurityEvent),
}

/// The typed channel a `WebApprovalGate` notifies through. It carries a
/// [`WebGateEvent`], deliberately NOT a text line and NOT the `StreamCallback`:
/// `ws.rs` converts each event into one `ChatStreamEvent` on the per-turn event channel.
///
/// Phase 54 gave this alias a text-sink type that was the same `StreamCallback` that
/// carries assistant tokens, which is how a bypass notice ended up mid-sentence inside
/// the assistant's answer (Phase 54.3 D-01). With a typed event there is no `&str` path
/// from the gate into any stream callback at all: the compiler refuses that wiring.
pub type NoticeSink = Arc<dyn Fn(&WebGateEvent) + Send + Sync>;

/// A second approval was requested for a session that already has one parked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AlreadyPending;

/// One parked approval. Lives only in the registry's in-memory map: the oneshot sender
/// is not serializable, so it can never be persisted.
struct PendingWebApproval {
    approval_id: String,
    responder: oneshot::Sender<bool>,
    /// A resolve after this instant is refused. Set slightly before the gate's own
    /// timeout starts, so a late answer can never win against a timeout.
    deadline: Instant,
}

/// The shared map of parked web approvals, one per session. Constructed once in
/// `AppState`; every turn's gate holds an `Arc` of it so the authenticated server
/// function can reach the gate that is waiting.
#[derive(Default)]
pub struct WebApprovalRegistry {
    pending: Mutex<HashMap<String, PendingWebApproval>>,
}

impl WebApprovalRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> MutexGuard<'_, HashMap<String, PendingWebApproval>> {
        self.pending.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Park a new approval for `session_id`, returning its random id and the receiver the
    /// gate awaits. `AlreadyPending` when this session already has a live one (D-02).
    /// An entry whose receiver is gone (a dropped turn) does not count as live.
    pub fn park(
        &self,
        session_id: &str,
        timeout: Duration,
    ) -> Result<(String, oneshot::Receiver<bool>), AlreadyPending> {
        let mut map = self.lock();
        if map
            .get(session_id)
            .is_some_and(|p| !p.responder.is_closed())
        {
            return Err(AlreadyPending);
        }
        // Either nothing is parked, or the previous waiter is gone (a dropped turn).
        map.remove(session_id);
        let approval_id = uuid::Uuid::new_v4().to_string();
        let (responder, receiver) = oneshot::channel();
        map.insert(
            session_id.to_string(),
            PendingWebApproval {
                approval_id: approval_id.clone(),
                responder,
                deadline: Instant::now() + timeout,
            },
        );
        Ok((approval_id, receiver))
    }

    /// Resolve the approval with this id. `false` when the id is unknown, already
    /// resolved, expired, or its waiter is gone; nothing changes in those cases.
    pub fn resolve_by_id(&self, approval_id: &str, approved: bool) -> bool {
        let mut map = self.lock();
        let Some(session_id) = map
            .iter()
            .find(|(_, p)| p.approval_id == approval_id)
            .map(|(session, _)| session.clone())
        else {
            return false;
        };
        Self::take_if_live(&mut map, &session_id)
            .is_some_and(|pending| pending.responder.send(approved).is_ok())
    }

    /// Remove and return the session's entry unless it is past its deadline. An expired
    /// entry is left in place: its own waiter is about to time out and must see
    /// `TimedOut`, not a closed channel. The check and the removal happen under the one
    /// lock the caller already holds, so an entry can be resolved at most once.
    fn take_if_live(
        map: &mut HashMap<String, PendingWebApproval>,
        session_id: &str,
    ) -> Option<PendingWebApproval> {
        if map
            .get(session_id)
            .is_some_and(|p| Instant::now() >= p.deadline)
        {
            return None;
        }
        map.remove(session_id)
    }

    /// Resolve the pending approval of exactly this session (the `ApprovalGate::resolve`
    /// contract). `false` when that session has none.
    pub fn resolve_by_session(&self, session_id: &str, approved: bool) -> bool {
        let mut map = self.lock();
        Self::take_if_live(&mut map, session_id)
            .is_some_and(|pending| pending.responder.send(approved).is_ok())
    }

    /// Drop the pending approval of `session_id`: its waiter sees a closed channel and
    /// resolves Denied with outcome `cancelled`.
    pub fn cancel(&self, session_id: &str) {
        self.lock().remove(session_id);
    }

    /// Remove the entry only if it still carries `approval_id` (a newer approval for the
    /// same session must survive an older one's cleanup).
    fn remove_if_id(&self, session_id: &str, approval_id: &str) {
        let mut map = self.lock();
        if map
            .get(session_id)
            .is_some_and(|p| p.approval_id == approval_id)
        {
            map.remove(session_id);
        }
    }

    /// Whether `session_id` has a parked approval. Tests only.
    #[cfg(test)]
    pub fn has_pending(&self, session_id: &str) -> bool {
        self.lock().contains_key(session_id)
    }
}

/// Removes a parked entry when the waiting future is dropped (a cancelled turn), so a
/// dead entry never blocks the session's next approval.
struct ParkedGuard<'a> {
    registry: &'a WebApprovalRegistry,
    session_id: &'a str,
    approval_id: &'a str,
}

impl Drop for ParkedGuard<'_> {
    fn drop(&mut self) {
        self.registry
            .remove_if_id(self.session_id, self.approval_id);
    }
}

/// Redact, strip control characters from and truncate a reason for the browser.
///
/// A word shaped `key=value` / `key:value` whose key names a secret keeps the key and
/// loses the value; the word after a bare `Bearer` is dropped. This is a backstop: the
/// reasons `execute_gated_command` builds name patterns and rule ids, not the command.
pub(crate) fn approval_reason_for_operator(reason: &str) -> String {
    let mut words: Vec<String> = Vec::new();
    let mut redact_next = false;
    for word in reason.split_whitespace() {
        if redact_next {
            words.push(REDACTED.to_string());
            redact_next = false;
            continue;
        }
        let lower = word.to_ascii_lowercase();
        if let Some((key, value)) = word.split_once(['=', ':']) {
            let key_lower = key.to_ascii_lowercase();
            if !key.is_empty() && SENSITIVE_WORD_TOKENS.iter().any(|t| key_lower.contains(t)) {
                if value.is_empty() {
                    words.push(word.to_string());
                    redact_next = true;
                } else {
                    words.push(format!("{key}={REDACTED}"));
                }
                continue;
            }
        }
        if lower == "bearer" {
            words.push(word.to_string());
            redact_next = true;
            continue;
        }
        words.push(word.to_string());
    }
    let joined: String = words
        .join(" ")
        .chars()
        .filter(|c| !c.is_control())
        .collect();
    if joined.chars().count() <= APPROVAL_REASON_MAX_CHARS {
        joined
    } else {
        let mut cut: String = joined.chars().take(APPROVAL_REASON_MAX_CHARS - 1).collect();
        cut.push('…');
        cut
    }
}

/// `ApprovalGate` for the web/desktop surface. See the module docs for the two postures.
///
/// `Copy` was dropped long ago (it holds `Arc`s); `Clone`/`Default` are kept. `Debug` is
/// hand-written because `dyn Fn` has none.
#[derive(Clone, Default)]
pub struct WebApprovalGate {
    notice_sink: Option<NoticeSink>,
    registry: Option<Arc<WebApprovalRegistry>>,
    timeout: Duration,
}

impl std::fmt::Debug for WebApprovalGate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WebApprovalGate")
            .field("notice_sink", &self.notice_sink.is_some())
            .field("interactive", &self.registry.is_some())
            .field("timeout", &self.timeout)
            .finish()
    }
}

impl WebApprovalGate {
    /// A gate with no sink and no registry: `request_approval` denies at once and the
    /// notifiers are silent no-ops.
    pub fn new() -> Self {
        Self::default()
    }

    /// A fail-closed gate wired with `sink` for notices. `request_approval` still denies
    /// at once (there is no registry to park in).
    pub fn with_notice_sink(sink: NoticeSink) -> Self {
        Self {
            notice_sink: Some(sink),
            registry: None,
            timeout: Duration::ZERO,
        }
    }

    /// The interactive gate (D-14): parks in `registry`, tells the browser through
    /// `sink`, and gives up after `timeout` (`approvals.timeout_secs`).
    pub fn interactive(
        registry: Arc<WebApprovalRegistry>,
        sink: NoticeSink,
        timeout: Duration,
    ) -> Self {
        Self {
            notice_sink: Some(sink),
            registry: Some(registry),
            timeout,
        }
    }

    /// The gate a turn's callers build: interactive when the turn has BOTH an event
    /// channel (`sink`) and the shared registry, otherwise fail-closed. A turn with no
    /// sink must not park: an approval nobody can see would only hang until the timeout.
    pub fn for_turn(
        sink: Option<NoticeSink>,
        approvals: Option<(Arc<WebApprovalRegistry>, Duration)>,
    ) -> Self {
        match (sink, approvals) {
            (Some(sink), Some((registry, timeout))) => Self::interactive(registry, sink, timeout),
            (Some(sink), None) => Self::with_notice_sink(sink),
            (None, _) => Self::new(),
        }
    }

    fn emit(&self, event: WebGateEvent) {
        if let Some(sink) = &self.notice_sink {
            (sink)(&event);
        }
    }
}

#[async_trait]
impl ApprovalGate for WebApprovalGate {
    async fn request_approval(
        &self,
        session_id: &str,
        tool_name: &str,
        reason: &str,
        _args: &serde_json::Value,
    ) -> ApprovalOutcome {
        let Some(registry) = &self.registry else {
            tracing::warn!(
                target: "ironhermes::security::web_approval_gate",
                session_id = %session_id,
                tool = %tool_name,
                reason = %reason,
                "web gate has no interactive registry for this caller — denying (fail-closed)"
            );
            return ApprovalOutcome::Denied;
        };
        let (approval_id, receiver) = match registry.park(session_id, self.timeout) {
            Ok(parked) => parked,
            Err(AlreadyPending) => {
                tracing::warn!(
                    target: "ironhermes::security::web_approval_gate",
                    session_id = %session_id,
                    tool = %tool_name,
                    "a web approval is already pending for this session — refusing the second"
                );
                return ApprovalOutcome::AlreadyPending;
            }
        };
        // Removes the entry if this future is dropped while parked (a cancelled turn).
        let _parked = ParkedGuard {
            registry,
            session_id,
            approval_id: &approval_id,
        };
        self.emit(WebGateEvent::ApprovalRequested {
            session_id: session_id.to_string(),
            approval_id: approval_id.clone(),
            tool: tool_name.to_string(),
            reason: approval_reason_for_operator(reason),
        });

        let (outcome, word) = match tokio::time::timeout(self.timeout, receiver).await {
            Ok(Ok(true)) => (ApprovalOutcome::Approved, "approved"),
            Ok(Ok(false)) => (ApprovalOutcome::Denied, "denied"),
            // The sender was dropped: the turn was cancelled or the entry replaced.
            Ok(Err(_)) => (ApprovalOutcome::Denied, "cancelled"),
            Err(_elapsed) => (ApprovalOutcome::TimedOut, "timed_out"),
        };
        registry.remove_if_id(session_id, &approval_id);
        self.emit(WebGateEvent::ApprovalResolved {
            session_id: session_id.to_string(),
            approval_id: approval_id.clone(),
            tool: tool_name.to_string(),
            outcome: word.to_string(),
        });
        outcome
    }

    async fn resolve(&self, session_id: &str, approved: bool) -> bool {
        match &self.registry {
            Some(registry) => registry.resolve_by_session(session_id, approved),
            None => false,
        }
    }

    /// Per-call unscoped fallback (a bypass reached outside any turn ledger). Builds a
    /// one-item [`YoloBypassSummary`] and pushes it through the same typed sink as the
    /// per-turn path. Best-effort: a gate with no sink is a silent no-op.
    async fn notify_yolo_bypass(&self, session_id: &str, tool_name: &str, reason: &str) {
        let summary = YoloBypassSummary::single(session_id, "web", "", tool_name, reason);
        self.emit(WebGateEvent::YoloBypass(summary));
    }

    /// Phase 54.3 (D-03/D-07): the per-turn flush. Hands the whole turn's summary to the
    /// typed sink exactly once.
    async fn notify_yolo_bypass_turn(&self, summary: &YoloBypassSummary) {
        self.emit(WebGateEvent::YoloBypass(summary.clone()));
    }

    /// Phase 57 D-16: pass the typed security event to the sink. Plan 14 renders it.
    async fn notify_security_event(&self, _session_id: &str, event: &SecurityEvent) {
        self.emit(WebGateEvent::Security(event.clone()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ironhermes_core::config::DefconLevel;
    use ironhermes_core::{AuditConfig, AuditLog};
    use ironhermes_hooks::{
        DangerousCommandGuardrail, GatePolicy, GatedOutcome, execute_gated_command,
    };
    use std::sync::atomic::{AtomicUsize, Ordering};

    // ---------------------------------------------------------------------
    // Harness: a registry, a recording sink, and an audit log in a temp dir.
    // ---------------------------------------------------------------------

    struct Harness {
        registry: Arc<WebApprovalRegistry>,
        events: Arc<Mutex<Vec<WebGateEvent>>>,
        audit: Arc<AuditLog>,
        audit_path: std::path::PathBuf,
        gate: Arc<WebApprovalGate>,
        _dir: tempfile::TempDir,
    }

    fn harness(timeout: Duration) -> Harness {
        let dir = tempfile::tempdir().unwrap();
        let audit_path = dir.path().join("audit.jsonl");
        let audit = Arc::new(AuditLog::with_path(
            audit_path.clone(),
            AuditConfig::default(),
        ));
        let registry = Arc::new(WebApprovalRegistry::new());
        let events: Arc<Mutex<Vec<WebGateEvent>>> = Arc::new(Mutex::new(Vec::new()));
        let sink_events = events.clone();
        let sink: NoticeSink = Arc::new(move |e: &WebGateEvent| {
            sink_events.lock().unwrap().push(e.clone());
        });
        let gate = Arc::new(WebApprovalGate::interactive(
            registry.clone(),
            sink,
            timeout,
        ));
        Harness {
            registry,
            events,
            audit,
            audit_path,
            gate,
            _dir: dir,
        }
    }

    impl Harness {
        /// Run `command` through the real chokepoint on a spawned task. `ran` counts how
        /// many times the executor closure actually ran.
        fn spawn_gated(
            &self,
            session: &str,
            command: &str,
            level: DefconLevel,
            ran: Arc<AtomicUsize>,
        ) -> tokio::task::JoinHandle<GatedOutcome> {
            let gate = self.gate.clone();
            let audit = self.audit.clone();
            let session = session.to_string();
            let command = command.to_string();
            tokio::spawn(async move {
                let guard = DangerousCommandGuardrail::new();
                execute_gated_command(
                    "terminal",
                    &command,
                    &guard,
                    Some(&*gate),
                    &audit,
                    &session,
                    "web",
                    &session,
                    GatePolicy::fixed(level, None),
                    false,
                    false,
                    || async move {
                        ran.fetch_add(1, Ordering::SeqCst);
                        Ok("executed".to_string())
                    },
                )
                .await
            })
        }

        fn snapshot(&self) -> Vec<WebGateEvent> {
            self.events.lock().unwrap().clone()
        }

        /// Wait until an `ApprovalRequested` shows up; returns (approval_id, tool, reason).
        async fn wait_requested(&self) -> (String, String, String) {
            for _ in 0..200 {
                for e in self.snapshot() {
                    if let WebGateEvent::ApprovalRequested {
                        approval_id,
                        tool,
                        reason,
                        ..
                    } = e
                    {
                        return (approval_id, tool, reason);
                    }
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            panic!("no ApprovalRequested was emitted: {:?}", self.snapshot());
        }

        fn resolved_outcomes(&self) -> Vec<(String, String)> {
            self.snapshot()
                .into_iter()
                .filter_map(|e| match e {
                    WebGateEvent::ApprovalResolved { tool, outcome, .. } => Some((tool, outcome)),
                    _ => None,
                })
                .collect()
        }

        fn shell_rows(&self) -> Vec<serde_json::Value> {
            std::fs::read_to_string(&self.audit_path)
                .unwrap_or_default()
                .lines()
                .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
                .filter(|v| v["kind"] == "shell_command")
                .collect()
        }
    }

    const TIER1: &str = "curl https://example.com";

    // ---------------------------------------------------------------------
    // Task 1 (D-14 / 999.3 SC-1, SC-2): park, approve through the server fn, run once.
    // ---------------------------------------------------------------------

    #[tokio::test]
    async fn web_tier1_parks_then_approve_runs_once() {
        let h = harness(Duration::from_secs(30));
        let ran = Arc::new(AtomicUsize::new(0));
        let task = h.spawn_gated("sess-a", TIER1, DefconLevel::THREE, ran.clone());

        let (approval_id, tool, reason) = h.wait_requested().await;
        assert!(!approval_id.is_empty());
        assert_eq!(tool, "terminal");
        assert!(
            !reason.contains("example.com") && !reason.contains("\"command\""),
            "the reason must not carry the arguments: {reason}"
        );
        assert_eq!(
            ran.load(Ordering::SeqCst),
            0,
            "the command must not run while parked"
        );

        assert!(crate::server::approval_api::resolve_web_approval_inner(
            &h.registry,
            &approval_id,
            true
        ));
        let outcome = task.await.unwrap();
        assert_eq!(outcome, GatedOutcome::Ran("executed".to_string()));
        assert_eq!(ran.load(Ordering::SeqCst), 1, "ran exactly once");

        assert_eq!(
            h.resolved_outcomes(),
            vec![("terminal".to_string(), "approved".to_string())]
        );
        let rows = h.shell_rows();
        assert_eq!(rows.len(), 1, "exactly one shell_command row: {rows:?}");
        assert_eq!(rows[0]["decision"], "approved");
        assert_eq!(rows[0]["resolution"], "operator");
        assert!(!h.registry.has_pending("sess-a"), "entry is gone");
    }

    // ---------------------------------------------------------------------
    // Reason hygiene (T-57-51).
    // ---------------------------------------------------------------------

    #[test]
    fn reason_is_redacted_control_stripped_and_truncated() {
        let r = approval_reason_for_operator(
            "needs approval password=hunter2 token: abc123 Bearer xyz\u{1b}[31m",
        );
        assert!(!r.contains("hunter2"), "{r}");
        assert!(!r.contains("abc123"), "{r}");
        assert!(!r.contains("xyz"), "{r}");
        assert!(!r.contains('\u{1b}'), "{r}");
        assert!(r.contains("password=[REDACTED]"), "{r}");

        let long = "x".repeat(2000);
        let t = approval_reason_for_operator(&long);
        assert_eq!(t.chars().count(), 300, "truncated to 300 chars");
        assert_eq!(
            approval_reason_for_operator("plain reason"),
            "plain reason",
            "benign text is unchanged"
        );
    }

    // ---------------------------------------------------------------------
    // Pre-existing behaviour, adapted to the typed WebGateEvent sink.
    // ---------------------------------------------------------------------

    /// A gate with no registry still fails closed.
    #[tokio::test]
    async fn request_approval_denies() {
        let gate = WebApprovalGate::new();
        let outcome = gate
            .request_approval(
                "sess-1",
                "terminal",
                "Tier-1 command requires approval",
                &serde_json::json!({ "command": "curl https://example.com" }),
            )
            .await;
        assert_eq!(
            outcome,
            ApprovalOutcome::Denied,
            "WebApprovalGate::new() must fail closed with Denied, never Approved"
        );
    }

    /// Never returns `Approved` and never panics across a variety of inputs.
    #[tokio::test]
    async fn request_approval_never_approves_across_varied_input() {
        let gate = WebApprovalGate::new();
        let cases: &[(&str, &str, &str, serde_json::Value)] = &[
            ("", "", "", serde_json::json!({})),
            (
                "sess-2",
                "execute_code",
                "",
                serde_json::json!({ "command": "" }),
            ),
            (
                "sess-3",
                "terminal",
                "remote backend — approval required regardless of guardrail classification (D-08)",
                serde_json::json!({ "command": "ls -la" }),
            ),
        ];
        for (session_id, tool_name, reason, args) in cases {
            let outcome = gate
                .request_approval(session_id, tool_name, reason, args)
                .await;
            assert_ne!(
                outcome,
                ApprovalOutcome::Approved,
                "must never approve for input ({session_id}, {tool_name}, {reason}, {args})"
            );
        }
    }

    /// `resolve` on a gate with no registry reports nothing was pending.
    #[tokio::test]
    async fn resolve_is_idempotent_noop() {
        let gate = WebApprovalGate::new();
        assert!(!gate.resolve("sess-1", true).await);
        assert!(!gate.resolve("sess-1", false).await);
    }

    fn capture_sink() -> (NoticeSink, Arc<Mutex<Vec<WebGateEvent>>>) {
        let captured: Arc<Mutex<Vec<WebGateEvent>>> = Arc::new(Mutex::new(Vec::new()));
        let clone = captured.clone();
        let sink: NoticeSink = Arc::new(move |e: &WebGateEvent| {
            clone.lock().unwrap().push(e.clone());
        });
        (sink, captured)
    }

    /// The per-call unscoped fallback delivers a one-item summary through the sink.
    #[tokio::test]
    async fn notify_yolo_bypass_reaches_notice_sink() {
        let (sink, captured) = capture_sink();
        let gate = WebApprovalGate::with_notice_sink(sink);

        gate.notify_yolo_bypass("sess-1", "execute_code", "dangerous pattern matched")
            .await;

        let captured = captured.lock().unwrap();
        assert_eq!(captured.len(), 1, "exactly one event, got {captured:?}");
        let WebGateEvent::YoloBypass(summary) = &captured[0] else {
            panic!("expected a YoloBypass event: {:?}", captured[0]);
        };
        assert_eq!(summary.count(), 1);
        assert!(
            summary
                .by_tool()
                .iter()
                .any(|(name, n)| name == "execute_code" && *n == 1),
            "summary must name the tool: {:?}",
            summary.by_tool()
        );
        assert!(
            summary
                .reasons_capped(20)
                .iter()
                .any(|r| r.contains("dangerous pattern matched")),
            "summary must contain the guardrail reason: {:?}",
            summary.reasons_capped(20)
        );
    }

    /// A wired sink does not change a registry-less gate's denial.
    #[tokio::test]
    async fn notice_sink_does_not_affect_request_approval_denial() {
        let (sink, captured) = capture_sink();
        let gate = WebApprovalGate::with_notice_sink(sink);
        let outcome = gate
            .request_approval(
                "sess-1",
                "terminal",
                "reason",
                &serde_json::json!({ "command": "ls" }),
            )
            .await;
        assert_eq!(outcome, ApprovalOutcome::Denied);
        assert!(
            captured.lock().unwrap().is_empty(),
            "a fail-closed denial asks nobody, so it emits nothing"
        );
    }

    /// V7a + V7b (Phase 54.3 D-01/D-07): the per-turn path delivers exactly ONE typed
    /// summary to the sink and ZERO bytes to any stream callback.
    #[tokio::test]
    async fn web_gate_notify_turn_delivers_one_summary_and_zero_stream_bytes() {
        let stream_bytes: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let stream_recorder: Box<dyn Fn(&str) + Send + Sync> = {
            let stream_bytes = stream_bytes.clone();
            Box::new(move |s: &str| stream_bytes.lock().unwrap().push(s.to_string()))
        };

        let (sink, captured) = capture_sink();
        let gate = WebApprovalGate::with_notice_sink(sink);

        let mut summary = YoloBypassSummary::single(
            "sess-1",
            "web",
            "",
            "terminal",
            "command contains shell metacharacters",
        );
        summary.items.extend(
            YoloBypassSummary::single("sess-1", "web", "", "execute_code", "dangerous pattern")
                .items,
        );

        gate.notify_yolo_bypass_turn(&summary).await;

        let captured = captured.lock().unwrap();
        assert_eq!(
            captured.len(),
            1,
            "a 2-item turn must produce exactly ONE event at the sink, got {captured:?}"
        );
        let WebGateEvent::YoloBypass(got) = &captured[0] else {
            panic!("expected YoloBypass: {:?}", captured[0]);
        };
        assert_eq!(got.count(), 2);
        let tools = got.by_tool();
        assert!(tools.iter().any(|(n, _)| n == "terminal"), "{tools:?}");
        assert!(tools.iter().any(|(n, _)| n == "execute_code"), "{tools:?}");

        // Referenced so the recorder is a live value, not optimised out of the test:
        // it must still be empty because nothing ever wrote to it.
        let _ = &stream_recorder;
        assert!(
            stream_bytes.lock().unwrap().is_empty(),
            "ZERO notice bytes may reach a StreamCallback (D-01/D-07)"
        );
    }

    /// Per-turn twin of `notify_yolo_bypass_without_sink_does_not_panic`.
    #[tokio::test]
    async fn web_gate_notify_turn_without_sink_does_not_panic() {
        let gate = WebApprovalGate::new();
        let summary = YoloBypassSummary::single("sess-1", "web", "", "terminal", "reason");
        gate.notify_yolo_bypass_turn(&summary).await;
    }

    /// `new()` (no sink wired) must be a silent no-op, never a panic.
    #[tokio::test]
    async fn notify_yolo_bypass_without_sink_does_not_panic() {
        let gate = WebApprovalGate::new();
        gate.notify_yolo_bypass("sess-1", "terminal", "reason")
            .await;
    }

    /// D-16: a security event reaches the sink as a typed `Security` event.
    #[tokio::test]
    async fn security_event_reaches_the_sink() {
        let (sink, captured) = capture_sink();
        let gate = WebApprovalGate::with_notice_sink(sink);
        let event = SecurityEvent {
            kind: ironhermes_core::SecurityEventKind::Blocked,
            tool: "terminal".into(),
            rule_ids: vec!["R".into()],
            level: 3,
            summary: "blocked at DEFCON 3".into(),
        };
        gate.notify_security_event("sess-1", &event).await;
        let captured = captured.lock().unwrap();
        assert!(
            matches!(&captured[..], [WebGateEvent::Security(e)] if *e == event),
            "{captured:?}"
        );
    }

    // ---------------------------------------------------------------------
    // Task 2 (D-14 / 999.3 SC-2..SC-5): every non-approval ends in a denial.
    // ---------------------------------------------------------------------

    /// Wait until the registry shows a parked approval for `session` (bounded).
    async fn wait_parked(h: &Harness, session: &str) {
        for _ in 0..200 {
            if h.registry.has_pending(session) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("nothing parked for {session}");
    }

    /// 999.3 SC-2: a deny refuses the command, runs nothing and is audited once.
    #[tokio::test]
    async fn web_deny_refuses_and_audits_once() {
        let h = harness(Duration::from_secs(30));
        let ran = Arc::new(AtomicUsize::new(0));
        let task = h.spawn_gated("sess-d", TIER1, DefconLevel::THREE, ran.clone());

        let (approval_id, _, _) = h.wait_requested().await;
        assert!(crate::server::approval_api::resolve_web_approval_inner(
            &h.registry,
            &approval_id,
            false
        ));
        let outcome = task.await.unwrap();
        assert!(matches!(outcome, GatedOutcome::Denied(_)), "{outcome:?}");
        assert_eq!(ran.load(Ordering::SeqCst), 0, "a denied command never runs");

        let rows = h.shell_rows();
        assert_eq!(rows.len(), 1, "exactly one shell_command row: {rows:?}");
        assert_eq!(rows[0]["decision"], "denied");
        assert_eq!(rows[0]["resolution"], "operator");
        assert_eq!(
            h.resolved_outcomes(),
            vec![("terminal".to_string(), "denied".to_string())]
        );
    }

    /// 999.3 SC-3: no answer inside `approvals.timeout_secs` denies.
    #[tokio::test]
    async fn web_no_answer_times_out_and_denies() {
        let h = harness(Duration::from_millis(200));
        let ran = Arc::new(AtomicUsize::new(0));
        let task = h.spawn_gated("sess-t", TIER1, DefconLevel::THREE, ran.clone());

        let outcome = task.await.unwrap();
        assert!(matches!(outcome, GatedOutcome::Denied(_)), "{outcome:?}");
        assert_eq!(
            ran.load(Ordering::SeqCst),
            0,
            "a timed-out command never runs"
        );

        let rows = h.shell_rows();
        assert_eq!(rows.len(), 1, "{rows:?}");
        assert_eq!(rows[0]["decision"], "denied");
        assert_eq!(rows[0]["resolution"], "timeout");
        assert_eq!(
            h.resolved_outcomes(),
            vec![("terminal".to_string(), "timed_out".to_string())]
        );
        assert!(!h.registry.has_pending("sess-t"), "the entry is removed");
    }

    /// 999.3 SC-4 / T-57-49: unknown, malformed and already-expired ids change nothing.
    #[tokio::test]
    async fn unknown_or_expired_id_rejected() {
        use crate::server::approval_api::resolve_web_approval_inner as resolve;

        // An approval that times out, then its id is replayed.
        let h = harness(Duration::from_millis(100));
        let ran = Arc::new(AtomicUsize::new(0));
        let task = h.spawn_gated("sess-x", TIER1, DefconLevel::THREE, ran.clone());
        let (expired_id, _, _) = h.wait_requested().await;
        let _ = task.await.unwrap();
        assert!(
            !resolve(&h.registry, &expired_id, true),
            "an id that already timed out must be refused"
        );

        // A live approval must survive every bogus attempt untouched.
        let live = harness(Duration::from_secs(30));
        let ran = Arc::new(AtomicUsize::new(0));
        let task = live.spawn_gated("sess-live", TIER1, DefconLevel::THREE, ran.clone());
        let (live_id, _, _) = live.wait_requested().await;
        assert!(!resolve(
            &live.registry,
            &uuid::Uuid::new_v4().to_string(),
            true
        ));
        assert!(!resolve(&live.registry, "not-a-uuid", true));
        assert!(!resolve(&live.registry, "", true));
        assert!(live.registry.has_pending("sess-live"), "still parked");
        assert_eq!(ran.load(Ordering::SeqCst), 0, "nothing ran");

        // A resolved id cannot be replayed.
        assert!(resolve(&live.registry, &live_id, true));
        let _ = task.await.unwrap();
        assert!(!resolve(&live.registry, &live_id, true), "replay refused");
        assert_eq!(ran.load(Ordering::SeqCst), 1, "ran exactly once");
    }

    /// T-57-49: an answer that arrives after the deadline loses, even while the entry
    /// is still in the map (the waiter has not yet woken to remove it).
    #[test]
    fn expired_deadline_refuses_resolution() {
        let registry = WebApprovalRegistry::new();
        let (id, mut rx) = registry.park("s", Duration::from_millis(20)).unwrap();
        std::thread::sleep(Duration::from_millis(60));
        assert!(!registry.resolve_by_id(&id, true), "past the deadline");
        assert!(!registry.resolve_by_session("s", true), "past the deadline");
        assert!(rx.try_recv().is_err(), "no decision was delivered");

        let (id2, mut rx2) = registry.park("s2", Duration::from_secs(60)).unwrap();
        assert!(registry.resolve_by_id(&id2, true));
        assert_eq!(rx2.try_recv(), Ok(true));
        assert!(!registry.resolve_by_id(&id2, true), "resolved at most once");
    }

    /// 999.3 SC-4: another session's id cannot resolve this session's approval.
    #[tokio::test]
    async fn other_session_id_cannot_resolve_by_session() {
        let h = harness(Duration::from_secs(30));
        let ran = Arc::new(AtomicUsize::new(0));
        let task = h.spawn_gated("sess-A", TIER1, DefconLevel::THREE, ran.clone());
        let (approval_id, _, _) = h.wait_requested().await;

        assert!(
            !h.gate.resolve("other-session", true).await,
            "a different session must not resolve session A's approval"
        );
        assert!(h.registry.has_pending("sess-A"), "A is still parked");
        assert_eq!(ran.load(Ordering::SeqCst), 0);

        // The matching session can (the ApprovalGate::resolve contract).
        assert!(h.gate.resolve("sess-A", false).await);
        let outcome = task.await.unwrap();
        assert!(matches!(outcome, GatedOutcome::Denied(_)), "{outcome:?}");
        let _ = approval_id;
    }

    /// 999.3 SC-4: a second request while one is parked is AlreadyPending, and the
    /// first is unaffected.
    #[tokio::test]
    async fn second_request_while_pending_is_already_pending() {
        let h = harness(Duration::from_secs(30));
        let first_ran = Arc::new(AtomicUsize::new(0));
        let first = h.spawn_gated("sess-p", TIER1, DefconLevel::THREE, first_ran.clone());
        let (approval_id, _, _) = h.wait_requested().await;

        let second_ran = Arc::new(AtomicUsize::new(0));
        let second = h
            .spawn_gated(
                "sess-p",
                "curl https://other.example",
                DefconLevel::THREE,
                second_ran.clone(),
            )
            .await
            .unwrap();
        assert!(matches!(second, GatedOutcome::Denied(_)), "{second:?}");
        assert_eq!(second_ran.load(Ordering::SeqCst), 0);
        assert!(
            h.shell_rows()
                .iter()
                .any(|r| r["decision"] == "denied" && r["resolution"] == "already_pending"),
            "the refusal is audited as already_pending: {:?}",
            h.shell_rows()
        );

        // The first request is still parked and still resolvable.
        assert!(h.registry.has_pending("sess-p"));
        assert!(crate::server::approval_api::resolve_web_approval_inner(
            &h.registry,
            &approval_id,
            true
        ));
        assert_eq!(
            first.await.unwrap(),
            GatedOutcome::Ran("executed".to_string())
        );
        assert_eq!(first_ran.load(Ordering::SeqCst), 1);
    }

    /// 999.3 SC-4: a dropped turn resolves Denied.
    #[tokio::test]
    async fn dropped_turn_denies() {
        // (a) the registry entry is cancelled (what /stop does).
        let h = harness(Duration::from_secs(30));
        let ran = Arc::new(AtomicUsize::new(0));
        let task = h.spawn_gated("sess-c", TIER1, DefconLevel::THREE, ran.clone());
        wait_parked(&h, "sess-c").await;
        h.registry.cancel("sess-c");
        let outcome = task.await.unwrap();
        assert!(matches!(outcome, GatedOutcome::Denied(_)), "{outcome:?}");
        assert_eq!(ran.load(Ordering::SeqCst), 0);
        assert_eq!(
            h.resolved_outcomes(),
            vec![("terminal".to_string(), "cancelled".to_string())]
        );

        // (b) the turn's future is dropped while parked: the entry does not linger,
        // so the session's next approval is not refused as AlreadyPending.
        let h = harness(Duration::from_secs(30));
        let ran = Arc::new(AtomicUsize::new(0));
        let task = h.spawn_gated("sess-c2", TIER1, DefconLevel::THREE, ran.clone());
        wait_parked(&h, "sess-c2").await;
        task.abort();
        let _ = task.await;
        assert!(
            !h.registry.has_pending("sess-c2"),
            "a dropped turn must not leave a parked entry behind"
        );
        assert!(h.registry.park("sess-c2", Duration::from_secs(1)).is_ok());
    }
}
