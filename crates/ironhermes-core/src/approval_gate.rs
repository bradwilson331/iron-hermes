//! Approval gate trait — async cross-crate seam for the gateway approval engine.
//!
//! Lives in `ironhermes-core` so `AgentLoop` (in `ironhermes-agent`, which depends on
//! core) can hold `Option<Arc<dyn ApprovalGate>>` while the gateway provides the
//! implementation, preserving the agent→core direction without introducing a cycle.
//!
//! # Phase 45 decision anchors
//!
//! - D-02: one active pending per session → [`ApprovalOutcome::AlreadyPending`]
//! - D-07: pending approvals are in-memory only; no disk persistence seam here
//! - Fail-closed: anything other than `Approved` is treated as a denial

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

/// The final decision returned by an approval gate after a `request_approval` call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalOutcome {
    /// The operator approved the action in time.
    Approved,
    /// The operator explicitly denied the action.
    Denied,
    /// The configured timeout elapsed before a decision was made (D-04/D-05).
    TimedOut,
    /// Another approval is already pending for this session (D-02).
    AlreadyPending,
    /// No gate is configured for this turn — treated as denied (fail-closed default).
    GateUnavailable,
}

/// Async trait for approval gate implementations.
///
/// The gateway implements this via `GatewayApprovalGate` (phase 45 plan 03); the agent
/// loop holds it as `Option<Arc<dyn ApprovalGate>>` so it can operate without a gate
/// in non-gateway contexts (e.g. CLI, test harness).
///
/// **Fail-closed rule:** callers MUST treat any outcome other than `Approved` as a
/// denial — never proceed with a destructive action on `GateUnavailable`.
#[async_trait]
pub trait ApprovalGate: Send + Sync {
    /// Park the caller until the operator approves/denies or the configured timeout elapses.
    ///
    /// Implementations must:
    /// - Return `AlreadyPending` immediately if this session already has a pending request (D-02).
    /// - Return `TimedOut` when `approvals.timeout_secs` elapses without a decision (D-04).
    /// - Notify the chat on expiry (D-05) before returning `TimedOut`.
    ///
    /// The `reason` string is surfaced to the operator in the approval prompt.
    ///
    /// `args` (Phase 46 D-03) is the raw tool-call/command arguments, threaded through
    /// so an audit-log implementation can record redacted, truncated arguments
    /// alongside the resolution. Implementations that do not audit may ignore it.
    async fn request_approval(
        &self,
        session_id: &str,
        tool_name: &str,
        reason: &str,
        args: &serde_json::Value,
    ) -> ApprovalOutcome;

    /// Resolve a pending approval for the given session.
    ///
    /// - `approved: true`  → unparks the waiting task with `ApprovalOutcome::Approved`.
    /// - `approved: false` → unparks the waiting task with `ApprovalOutcome::Denied`.
    ///
    /// Returns `false` if no pending approval exists for `session_id` (idempotent: safe
    /// to call on `/deny` when there is no pending request).
    async fn resolve(&self, session_id: &str, approved: bool) -> bool;

    /// Notify the operator that a `Warn`/forced-approval decision was bypassed under
    /// `yolo = true` (D-04, Phase 54 Finding 1). Called from
    /// `ironhermes_hooks::gated_exec`'s yolo branch INSTEAD of `request_approval` —
    /// under yolo the decision is never actually put to the operator for approval,
    /// but the operator must still learn it happened.
    ///
    /// The default no-op body is deliberate: it is what lets every existing
    /// implementor of this trait (including test doubles) keep compiling unchanged
    /// after this method is added, at zero call-site cost. Each surface's real gate
    /// overrides it to push `reason` onto its own transcript/chat channel — CLI
    /// stderr, the TUI transcript, the gateway's chat-delivery path, or (Phase 54.3
    /// D-01/D-07) a typed `ChatStreamEvent` delivered on the web websocket event
    /// channel. The web `StreamCallback` that carries assistant tokens is NOT a
    /// separate channel for this purpose. `AcpApprovalGate` and
    /// `ImmediateDenyApprovalGate` deliberately keep the default (see 54-02's
    /// SUMMARY.md "Scoping decisions").
    ///
    /// **The fail-closed contract above is unaffected by this method.** It returns
    /// `()` and is never consulted to decide whether the gated command runs — only
    /// whether the operator is told that it did. Best-effort: implementations should
    /// not let a slow or failing notification delay or block command execution, and
    /// callers must tolerate `approval_gate` being `None` (no notification, no panic).
    async fn notify_yolo_bypass(&self, _session_id: &str, _tool_name: &str, _reason: &str) {}

    /// Phase 54.3 D-03: deliver ONE aggregated notice for every yolo bypass that
    /// happened inside a single agent turn, instead of one notice per bypassed call.
    ///
    /// `AgentRuntime::run_turn` scopes a per-turn ledger around the agent run; the
    /// `gated_exec` chokepoint records each bypass into it; the turn boundary flushes
    /// the ledger once and calls this method with the totals. The per-call audit rows
    /// (`resolution = "bypass"`, D-04) are unaffected — only the *operator notice* is
    /// aggregated.
    ///
    /// **Default body = the fail-visible bridge.** It delegates to
    /// [`notify_yolo_bypass`](Self::notify_yolo_bypass) once per recorded item. An
    /// implementor that has not yet overridden this method therefore still tells the
    /// operator about every bypass, per-call, exactly as before — never silently.
    /// [`notify_yolo_bypass`](Self::notify_yolo_bypass) is retained as the per-call
    /// entry point for the unscoped fallback (a bypass reached outside any turn
    /// ledger) and is NOT deprecated.
    ///
    /// An override of this method MUST NOT call back into
    /// `notify_yolo_bypass_turn` (the default delegates the other way; a cycle would
    /// recurse forever). Same best-effort contract as `notify_yolo_bypass`: returns
    /// `()`, never consulted to decide whether anything runs, and the flusher bounds
    /// the await with a timeout.
    async fn notify_yolo_bypass_turn(&self, summary: &YoloBypassSummary) {
        for it in &summary.items {
            self.notify_yolo_bypass(&summary.session_id, &it.tool, &it.reason)
                .await;
        }
    }

    /// Phase 57 D-16: report a security event (a Block, a detected credential, a sidecar
    /// outage, a Warn that ran, a bypass) to the surface that issued the call, so it can
    /// render it (plans 10, 12, 13 and 17 do).
    ///
    /// The default no-op body is deliberate, for the same reason as
    /// [`notify_yolo_bypass`](Self::notify_yolo_bypass): every existing implementor keeps
    /// compiling and behaving as before. Same best-effort contract: returns `()`, is never
    /// consulted to decide whether anything runs, and callers bound the await with a
    /// timeout. `event` never carries command text, arguments or secret values.
    async fn notify_security_event(&self, _session_id: &str, _event: &SecurityEvent) {}
}

/// What happened, for [`SecurityEvent`] (Phase 57 D-16).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SecurityEventKind {
    /// A call was hard-blocked.
    Blocked,
    /// An approval was put to the operator.
    ApprovalRequested,
    /// The operator answered (or the request timed out).
    ApprovalResolved,
    /// A tool failed in a way that ended the turn.
    ToolFailedTurnEnded,
    /// A credential-shaped value was detected in screened input.
    CredentialDetected,
    /// A background job (cron, kanban) was blocked outside any turn.
    JobBlocked,
    /// The detection sidecar could not answer.
    SidecarUnavailable,
    /// A session's DEFCON level changed.
    DefconChanged,
    /// A Warn ran after the operator approved it.
    WarnRan,
    /// A Warn ran without approval at a bypass-active level (DEFCON 4-5).
    BypassRan,
}

impl SecurityEventKind {
    /// Whether the surface should treat this as critical (D-16/D-17). `WarnRan` and
    /// `BypassRan` only colour the web dot amber; every other kind is critical.
    pub fn is_critical(&self) -> bool {
        !matches!(self, Self::WarnRan | Self::BypassRan)
    }
}

/// One security event, as delivered to [`ApprovalGate::notify_security_event`] or the
/// process-local bus ([`publish_security_event`]).
///
/// `summary` is short operator-facing text. It never carries command text, arguments or
/// secret values: the same rule as the audit `security_verdict` row.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SecurityEvent {
    pub kind: SecurityEventKind,
    /// The tool the event is about (`terminal`, `execute_code`, an MCP `server__tool`, ...).
    pub tool: String,
    /// Rule ids that produced the event. When no named rule decided a Block, the list holds
    /// one cause marker instead (see `content_screen`): `regex_floor` (the built-in rules
    /// decided), `unscreened:<reason>` (the sidecar could not screen the subject) or
    /// `sidecar_outage:<reason>` (the sidecar was down). Markers appear only here, never in
    /// an audit row. A producer that has no id at all sends an empty list.
    pub rule_ids: Vec<String>,
    /// DEFCON level (1-5) in force when the event happened.
    pub level: u8,
    pub summary: String,
}

/// Capacity of the process-local security-event bus. A subscriber that falls this far
/// behind loses the oldest events (`RecvError::Lagged`), never blocks a publisher.
const SECURITY_EVENT_BUS_CAPACITY: usize = 256;

static SECURITY_EVENT_BUS: std::sync::LazyLock<tokio::sync::broadcast::Sender<SecurityEvent>> =
    std::sync::LazyLock::new(|| tokio::sync::broadcast::channel(SECURITY_EVENT_BUS_CAPACITY).0);

/// Publish a security event raised OUTSIDE any turn (D-16 `JobBlocked`, from cron and
/// kanban jobs: plan 11). In-turn events go through the turn's [`ApprovalGate`]; this bus
/// carries only the events that have no gate in scope.
///
/// Process-local: there is no cross-process delivery, and a host forwards what it receives
/// (plan 14). Publishing with no subscriber is not an error.
pub fn publish_security_event(event: &SecurityEvent) {
    // `send` errs only when there is no receiver; that is a normal state, not a failure.
    let _ = SECURITY_EVENT_BUS.send(event.clone());
}

/// Subscribe to events published with [`publish_security_event`]. Each receiver sees every
/// event published after it subscribed.
pub fn subscribe_security_events() -> tokio::sync::broadcast::Receiver<SecurityEvent> {
    SECURITY_EVENT_BUS.subscribe()
}

/// Maximum chars kept per operator-facing notice field (T-45-10/F4, promoted from
/// the gateway so CLI, TUI and web inherit it — Phase 54.3 T-54.3-01).
const NOTICE_FIELD_MAX_CHARS: usize = 200;

/// Hard cap on [`YoloBypassSummary::short_line`] length, in chars.
const SHORT_LINE_MAX_CHARS: usize = 200;

/// Sanitize one untrusted text field (a bypassed tool name can be remote-server
/// supplied) before it is interpolated into any operator-facing notice: `\n`/`\r`
/// become a space, every other control char is dropped, and the result is capped at
/// 200 chars. This is the gateway's T-45-10/F4 block, promoted to core.
pub fn sanitize_notice_field(s: &str) -> String {
    s.chars()
        .filter_map(|c| match c {
            '\n' | '\r' => Some(' '),
            c if c.is_control() => None,
            c => Some(c),
        })
        .take(NOTICE_FIELD_MAX_CHARS)
        .collect()
}

/// One yolo-bypassed tool call (Phase 54.3).
#[derive(Clone, Debug)]
pub struct YoloBypass {
    /// Tool name (`terminal`, `execute_code`, an MCP `server__tool`, ...). Untrusted.
    pub tool: String,
    /// The guardrail reason that yolo overrode. Untrusted.
    pub reason: String,
}

/// Everything one turn bypassed under yolo — the unit of delivery for
/// [`ApprovalGate::notify_yolo_bypass_turn`] (Phase 54.3 D-03).
///
/// The methods below are the shared vocabulary every surface formatter consumes;
/// no surface reimplements them.
#[derive(Clone, Debug, Default)]
pub struct YoloBypassSummary {
    /// Session the turn ran under.
    pub session_id: String,
    /// Originating surface label (`"tui"`, `"cli-repl"`, `"telegram"`, ...).
    pub surface: String,
    /// Originating chat id.
    pub chat_id: String,
    /// Stored detail, one per bypass (capped by the ledger in a runaway turn).
    pub items: Vec<YoloBypass>,
    /// Bypasses recorded beyond the stored-detail cap. Keeps [`count`](Self::count)
    /// exact (so the rollup audit row's `count` equals the number of
    /// `resolution = "bypass"` rows) even when `items` was capped.
    pub overflow: u32,
}

impl YoloBypassSummary {
    /// A one-item summary — the unscoped-fallback shape.
    pub fn single(
        session_id: &str,
        surface: &str,
        chat_id: &str,
        tool: &str,
        reason: &str,
    ) -> Self {
        Self {
            session_id: session_id.to_string(),
            surface: surface.to_string(),
            chat_id: chat_id.to_string(),
            items: vec![YoloBypass {
                tool: tool.to_string(),
                reason: reason.to_string(),
            }],
            overflow: 0,
        }
    }

    /// Exact number of bypasses in the turn.
    pub fn count(&self) -> u32 {
        (self.items.len() as u32).saturating_add(self.overflow)
    }

    /// Unique (sanitized) tool names with occurrence counts, sorted by descending
    /// count then by name for a stable order. Counts cover the stored detail only.
    pub fn by_tool(&self) -> Vec<(String, u32)> {
        let mut counts: std::collections::BTreeMap<String, u32> = std::collections::BTreeMap::new();
        for it in &self.items {
            *counts.entry(sanitize_notice_field(&it.tool)).or_insert(0) += 1;
        }
        let mut v: Vec<(String, u32)> = counts.into_iter().collect();
        v.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        v
    }

    /// De-duplicated (sanitized) reasons in first-seen order, at most `max` of them.
    pub fn reasons_capped(&self, max: usize) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for it in &self.items {
            if out.len() >= max {
                break;
            }
            let r = sanitize_notice_field(&it.reason);
            if !out.contains(&r) {
                out.push(r);
            }
        }
        out
    }

    /// The one-line per-turn notice text, shaped
    /// `⚡ {count} ran unapproved: {tool} ×{n}, {tool} ×{n}`. Contains no `\n`/`\r` and
    /// is at most 200 chars; the tool list (never mid-name) is truncated with a
    /// trailing `…` to stay under the cap. The single source of the "same short
    /// content" D-03 requires on every surface.
    pub fn short_line(&self) -> String {
        let prefix = format!("\u{26a1} {} ran unapproved: ", self.count());
        let parts: Vec<String> = self
            .by_tool()
            .into_iter()
            .map(|(name, n)| format!("{name} \u{d7}{n}"))
            .collect();
        let mut keep = parts.len();
        loop {
            let mut line = format!("{prefix}{}", parts[..keep].join(", "));
            if keep < parts.len() {
                line.push_str(" \u{2026}");
            }
            if line.chars().count() <= SHORT_LINE_MAX_CHARS {
                return line;
            }
            if keep == 0 {
                // Even the bare prefix + ellipsis does not fit (impossible for a u32
                // count, but never exceed the cap): hard-truncate.
                let mut hard: String = line.chars().take(SHORT_LINE_MAX_CHARS - 1).collect();
                hard.push('\u{2026}');
                return hard;
            }
            keep -= 1;
        }
    }
}

#[cfg(test)]
mod yolo_summary_tests {
    use super::*;

    #[test]
    fn sanitize_strips_newlines_controls_and_caps_length() {
        assert_eq!(sanitize_notice_field("a\nb\rc\u{7}d"), "a b cd");
        assert_eq!(sanitize_notice_field(&"x".repeat(500)).chars().count(), 200);
    }

    #[test]
    fn short_line_is_single_line_and_capped() {
        let mut s = YoloBypassSummary::single("s", "tui", "c", "terminal", "r");
        for i in 0..400 {
            s.items.push(YoloBypass {
                tool: format!("srv__tool_{i}\n"),
                reason: "r".into(),
            });
        }
        let line = s.short_line();
        assert!(!line.contains('\n') && !line.contains('\r'));
        assert!(line.chars().count() <= 200, "len {}", line.chars().count());
        assert!(line.starts_with("\u{26a1} 401 ran unapproved: "));
    }

    #[test]
    fn short_line_groups_by_tool_with_counts() {
        let mut s = YoloBypassSummary::single("s", "tui", "c", "terminal", "r1");
        s.items.push(YoloBypass {
            tool: "terminal".into(),
            reason: "r2".into(),
        });
        assert_eq!(
            s.short_line(),
            "\u{26a1} 2 ran unapproved: terminal \u{d7}2"
        );
        assert_eq!(s.reasons_capped(1), vec!["r1".to_string()]);
    }

    #[test]
    fn overflow_keeps_count_exact() {
        let mut s = YoloBypassSummary::single("s", "tui", "c", "terminal", "r");
        s.overflow = 9;
        assert_eq!(s.count(), 10);
    }
}

#[cfg(test)]
mod security_event_tests {
    use super::*;

    fn event(kind: SecurityEventKind, tool: &str) -> SecurityEvent {
        SecurityEvent {
            kind,
            tool: tool.to_string(),
            rule_ids: vec!["mutation_command".to_string(), "FakeBlock".to_string()],
            level: 3,
            summary: "blocked at DEFCON 3".to_string(),
        }
    }

    /// Receivers can see events from other tests in this process; match on a unique tool.
    fn recv_for(
        rx: &mut tokio::sync::broadcast::Receiver<SecurityEvent>,
        tool: &str,
    ) -> Option<SecurityEvent> {
        while let Ok(ev) = rx.try_recv() {
            if ev.tool == tool {
                return Some(ev);
            }
        }
        None
    }

    /// D-16: two subscribers both receive one published event unchanged, publishing with
    /// nobody listening neither panics nor errors, and a JobBlocked event round-trips.
    #[test]
    fn process_security_bus_fans_out() {
        // No subscriber for this tool yet: must be silent.
        publish_security_event(&event(SecurityEventKind::JobBlocked, "bus-test-nobody"));

        let mut a = subscribe_security_events();
        let mut b = subscribe_security_events();
        let sent = event(SecurityEventKind::JobBlocked, "bus-test-job");
        publish_security_event(&sent);

        let got_a = recv_for(&mut a, "bus-test-job").expect("subscriber a receives");
        let got_b = recv_for(&mut b, "bus-test-job").expect("subscriber b receives");
        assert_eq!(got_a, sent);
        assert_eq!(got_b, sent);
        assert_eq!(got_a.kind, SecurityEventKind::JobBlocked);
        assert_eq!(got_a.tool, "bus-test-job");
        assert_eq!(got_a.rule_ids, sent.rule_ids);
        assert_eq!(got_a.level, 3);
        assert_eq!(got_a.summary, "blocked at DEFCON 3");

        // The "nobody listening" publish happened before subscribing: not replayed.
        assert!(recv_for(&mut a, "bus-test-nobody").is_none());
    }

    /// D-16/D-17: only WarnRan and BypassRan are non-critical.
    #[test]
    fn only_warn_ran_and_bypass_ran_are_non_critical() {
        use SecurityEventKind::*;
        for kind in [
            Blocked,
            ApprovalRequested,
            ApprovalResolved,
            ToolFailedTurnEnded,
            CredentialDetected,
            JobBlocked,
            SidecarUnavailable,
            DefconChanged,
        ] {
            assert!(kind.is_critical(), "{kind:?}");
        }
        assert!(!WarnRan.is_critical());
        assert!(!BypassRan.is_critical());
    }

    /// The event serializes without any command-shaped field.
    #[test]
    fn security_event_round_trips_through_json() {
        let ev = event(SecurityEventKind::SidecarUnavailable, "terminal");
        let json = serde_json::to_string(&ev).expect("serialize");
        let back: SecurityEvent = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, ev);
    }
}
