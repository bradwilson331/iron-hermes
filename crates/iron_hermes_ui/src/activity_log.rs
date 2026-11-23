//! Phase 54.4 Plan 07 (D-08 / D-25): the client-side agent-activity ring.
//!
//! `HermesApp`'s WebSocket receive loop pushes one [`ActivityRow`] per
//! turn/tool/error/yolo/subagent/critical/approval event into a root-provided
//! [`ActivityLogCtx`] ring (cap [`ACTIVITY_RING_CAP`]). The Logs screen's AGENT
//! ACTIVITY tab (and plan 11's Logs mini-panel) read it reactively; nothing polls.
//!
//! The ring is complete: routine `ToolStart`/`ToolEnd` rows are recorded so the Logs screen
//! keeps them. Only the mini-panel filters, with [`ActivityEventKind::is_critical`]
//! (Phase 57 D-18): the rail shows Critical, Approval and Error rows.
//!
//! # Attribution
//!
//! Every row comes from the single embedded runtime, so its agent is the live
//! agent passed in by the caller. No per-event agent is invented.
//!
//! # No secrets (T-54.4-24)
//!
//! Rows carry tool names, counts, turn boundaries and the server-redacted summary of a
//! critical event only. Yolo `reasons` and an approval `reason` are never read, so they
//! cannot reach the client ring; tool arguments no longer exist on the wire.
//! Streaming content (`Delta`, `Finished`) is never recorded.

use crate::protocol::ChatStreamEvent;
use dioxus::prelude::*;

/// Maximum rows kept in the ring.
pub const ACTIVITY_RING_CAP: usize = 500;

/// Maximum characters in a row summary.
const SUMMARY_MAX_CHARS: usize = 240;

/// What happened.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ActivityEventKind {
    TurnStarted,
    TurnEnded,
    TurnCancelled,
    ToolStart,
    ToolEnd,
    Error,
    YoloBypass,
    Subagent,
    /// Phase 57 C4: a critical security event (block, scanner outage, credential, ...).
    Critical,
    /// Phase 57 C4: an approval request or its outcome.
    Approval,
}

impl ActivityEventKind {
    /// Whether the activity rail (the Logs mini-panel) lists this row (Phase 57 D-18 /
    /// UI-SPEC C4): Critical, Approval and Error rows. A successful tool end is never
    /// critical; a turn-ending failure arrives as an `Error` row. The Logs screen is not
    /// filtered by this.
    pub fn is_critical(self) -> bool {
        matches!(
            self,
            ActivityEventKind::Critical | ActivityEventKind::Approval | ActivityEventKind::Error
        )
    }

    /// The short uppercase label shown in the AGENT ACTIVITY row.
    pub fn label(self) -> &'static str {
        match self {
            ActivityEventKind::Critical => "CRITICAL",
            ActivityEventKind::Approval => "APPROVAL",
            ActivityEventKind::TurnStarted => "TURN START",
            ActivityEventKind::TurnEnded => "TURN END",
            ActivityEventKind::TurnCancelled => "TURN CANCEL",
            ActivityEventKind::ToolStart => "TOOL START",
            ActivityEventKind::ToolEnd => "TOOL END",
            ActivityEventKind::Error => "ERROR",
            ActivityEventKind::YoloBypass => "YOLO",
            ActivityEventKind::Subagent => "SUBAGENT",
        }
    }
}

/// One activity row.
#[derive(Clone, PartialEq, Debug)]
pub struct ActivityRow {
    /// Monotonic sequence number (stable row identity across ring caps).
    pub seq: u64,
    /// Local `HH:MM:SS` when the event arrived.
    pub hms: String,
    /// The live agent that produced the event.
    pub agent: String,
    pub kind: ActivityEventKind,
    pub summary: String,
}

/// Cut `s` to at most `max` characters; when it was longer, the last kept
/// character is replaced by an ellipsis so the cut is visible. Char-boundary
/// safe.
fn cut_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// Build the activity row for a stream event, or `None` for events that are
/// never recorded.
pub fn activity_row_for(
    ev: &ChatStreamEvent,
    agent: &str,
    hms: String,
    seq: u64,
) -> Option<ActivityRow> {
    let (kind, summary) = match ev {
        ChatStreamEvent::TurnStarted { session_id, .. } => (
            ActivityEventKind::TurnStarted,
            format!(
                "turn started · session {}",
                session_id.chars().take(8).collect::<String>()
            ),
        ),
        ChatStreamEvent::TurnEnded { .. } => {
            (ActivityEventKind::TurnEnded, "turn ended".to_string())
        }
        ChatStreamEvent::TurnCancelled { .. } => {
            (ActivityEventKind::TurnCancelled, "turn cancelled".to_string())
        }
        // The tool name only: `args` can carry secrets and is never read.
        ChatStreamEvent::ToolCallStart { name, .. } => {
            (ActivityEventKind::ToolStart, name.clone())
        }
        ChatStreamEvent::ToolCallEnd { name, success, .. } => (
            ActivityEventKind::ToolEnd,
            if *success {
                format!("{name} ok")
            } else {
                format!("{name} failed")
            },
        ),
        ChatStreamEvent::Error { message, .. } => (ActivityEventKind::Error, message.clone()),
        // Tool names and counts only: `reasons` can carry command text.
        ChatStreamEvent::YoloBypassNotice { count, tools, .. } => {
            let by_tool = tools
                .iter()
                .map(|(tool, n)| format!("{tool} ×{n}"))
                .collect::<Vec<_>>()
                .join(", ");
            let summary = if by_tool.is_empty() {
                format!("{count} ran unapproved")
            } else {
                format!("{count} ran unapproved: {by_tool}")
            };
            (ActivityEventKind::YoloBypass, summary)
        }
        ChatStreamEvent::SubagentEvent {} => {
            (ActivityEventKind::Subagent, "subagent activity".to_string())
        }
        // Phase 57 C4: the server-built, redacted summary of a critical event.
        ChatStreamEvent::CriticalEvent { summary, .. } => {
            (ActivityEventKind::Critical, summary.clone())
        }
        // Phase 57 C4: tool names only; the approval reason is never read.
        ChatStreamEvent::ApprovalRequested { tool, .. } => {
            (ActivityEventKind::Approval, format!("{tool} requested"))
        }
        ChatStreamEvent::ApprovalResolved { tool, outcome, .. } => {
            let verb = match outcome.as_str() {
                "approved" => "approved",
                "denied" => "denied",
                "timed_out" => "timed out",
                "cancelled" => "cancelled",
                // An unknown outcome is not shown as a decision nobody made.
                _ => return None,
            };
            (ActivityEventKind::Approval, format!("{tool} {verb}"))
        }
        // Delta, Finished, QueueUpdated, TurnSeverity, DefconLevel and the
        // voice/audio/image/video frames are never recorded.
        _ => return None,
    };
    Some(ActivityRow {
        seq,
        hms,
        agent: agent.to_string(),
        kind,
        summary: cut_chars(&summary, SUMMARY_MAX_CHARS),
    })
}

/// Push `row`, dropping the oldest rows past `cap`.
pub fn push_capped(ring: &mut Vec<ActivityRow>, row: ActivityRow, cap: usize) {
    ring.push(row);
    if ring.len() > cap {
        let excess = ring.len() - cap;
        ring.drain(..excess);
    }
}

/// Local wall-clock `HH:MM:SS` (browser clock on wasm, system clock natively).
pub fn now_hms() -> String {
    #[cfg(target_arch = "wasm32")]
    {
        let d = js_sys::Date::new_0();
        format!(
            "{:02}:{:02}:{:02}",
            d.get_hours(),
            d.get_minutes(),
            d.get_seconds()
        )
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        chrono::Local::now().format("%H:%M:%S").to_string()
    }
}

/// The root-provided activity ring. Provided ONLY in `HermesApp`'s own body: a
/// provider declared in a child compiles and then panics consumers outside its
/// subtree at runtime.
#[allow(dead_code)] // field read via `.0` at the consume sites; dead_code fires on isolated compilation units
#[derive(Clone, Copy)]
pub struct ActivityLogCtx(pub Signal<Vec<ActivityRow>>);

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn row(seq: u64) -> ActivityRow {
        ActivityRow {
            seq,
            hms: "00:00:00".to_string(),
            agent: "default".to_string(),
            kind: ActivityEventKind::TurnEnded,
            summary: format!("row {seq}"),
        }
    }

    fn rec(ev: &ChatStreamEvent) -> Option<ActivityRow> {
        activity_row_for(ev, "dev", "01:02:03".to_string(), 7)
    }

    #[test]
    fn activity_ring_records_turn_tool_error_yolo_and_subagent_events() {
        let id = Uuid::nil();
        let events = [
            ChatStreamEvent::TurnStarted {
                turn_id: id,
                session_id: "sess-abcdef123456".to_string(),
                index: 0,
            },
            ChatStreamEvent::TurnEnded {
                turn_id: id,
                session_id: "s".to_string(),
            },
            ChatStreamEvent::TurnCancelled { turn_id: id },
            ChatStreamEvent::ToolCallStart {
                turn_id: id,
                name: "terminal".to_string(),
            },
            ChatStreamEvent::ToolCallEnd {
                turn_id: id,
                name: "terminal".to_string(),
                success: true,
            },
            ChatStreamEvent::Error {
                turn_id: id,
                message: "boom".to_string(),
            },
            ChatStreamEvent::YoloBypassNotice {
                turn_id: id,
                session_id: "s".to_string(),
                count: 5,
                tools: vec![("terminal".to_string(), 5)],
                reasons: vec![],
            },
            ChatStreamEvent::SubagentEvent {},
        ];
        let kinds = [
            ActivityEventKind::TurnStarted,
            ActivityEventKind::TurnEnded,
            ActivityEventKind::TurnCancelled,
            ActivityEventKind::ToolStart,
            ActivityEventKind::ToolEnd,
            ActivityEventKind::Error,
            ActivityEventKind::YoloBypass,
            ActivityEventKind::Subagent,
        ];
        for (ev, kind) in events.iter().zip(kinds) {
            let r = rec(ev).unwrap_or_else(|| panic!("{kind:?} must be recorded"));
            assert_eq!(r.agent, "dev");
            assert_eq!(r.kind, kind);
            assert_eq!(r.hms, "01:02:03");
            assert_eq!(r.seq, 7);
            assert!(!r.summary.is_empty());
        }
        let started = rec(&events[0]).unwrap();
        assert_eq!(started.summary, "turn started · session sess-abc");
        let yolo = rec(&events[6]).unwrap();
        assert_eq!(yolo.summary, "5 ran unapproved: terminal ×5");
        assert_eq!(rec(&events[4]).unwrap().summary, "terminal ok");
    }

    #[test]
    fn activity_ring_never_records_delta_or_finished() {
        let id = Uuid::nil();
        assert!(rec(&ChatStreamEvent::Delta {
            turn_id: id,
            text: "hello".to_string()
        })
        .is_none());
        assert!(rec(&ChatStreamEvent::Finished {
            turn_id: id,
            total_tokens: 10
        })
        .is_none());
        assert!(rec(&ChatStreamEvent::QueueUpdated {
            depth: 1,
            paused: false
        })
        .is_none());
    }

    #[test]
    fn activity_ring_never_records_tool_args_or_yolo_reasons() {
        let id = Uuid::nil();
        // An older server's payload still carries `args`; it deserializes and the value is
        // dropped, so it cannot reach the ring.
        let old_wire = r#"{"ToolCallStart":{"turn_id":"00000000-0000-0000-0000-000000000000","name":"terminal","args":"{\"cmd\":\"export KEY=sk-SECRET\"}"}}"#;
        let ev: ChatStreamEvent = serde_json::from_str(old_wire).unwrap();
        let tool = rec(&ev).unwrap();
        assert!(!tool.summary.contains("sk-SECRET"));
        assert_eq!(tool.summary, "terminal");
        let yolo = rec(&ChatStreamEvent::YoloBypassNotice {
            turn_id: id,
            session_id: "s".to_string(),
            count: 1,
            tools: vec![("terminal".to_string(), 1)],
            reasons: vec!["SECRET-REASON".to_string()],
        })
        .unwrap();
        assert!(!yolo.summary.contains("SECRET-REASON"));
    }

    #[test]
    fn activity_ring_caps_at_500() {
        let mut ring: Vec<ActivityRow> = Vec::new();
        for i in 0..510 {
            push_capped(&mut ring, row(i), ACTIVITY_RING_CAP);
        }
        assert_eq!(ring.len(), 500);
        assert_eq!(ring.first().map(|r| r.seq), Some(10));
        assert_eq!(ring.last().map(|r| r.seq), Some(509));
        assert!(ring.windows(2).all(|w| w[0].seq < w[1].seq));
    }

    #[test]
    fn activity_ring_cuts_long_error_text() {
        let long = "é".repeat(1000);
        let r = rec(&ChatStreamEvent::Error {
            turn_id: Uuid::nil(),
            message: long,
        })
        .unwrap();
        assert!(r.summary.chars().count() <= 240);
        assert!(r.summary.ends_with('…'));
    }

    /// C4: Critical, Approval and Error rows are critical; routine rows are not.
    #[test]
    fn activity_kind_is_critical_classification() {
        use ActivityEventKind as K;
        for k in [K::Critical, K::Approval, K::Error] {
            assert!(k.is_critical(), "{k:?} must be critical");
        }
        for k in [
            K::TurnStarted,
            K::TurnEnded,
            K::TurnCancelled,
            K::ToolStart,
            K::ToolEnd,
            K::YoloBypass,
            K::Subagent,
        ] {
            assert!(!k.is_critical(), "{k:?} must not be critical");
        }
        assert_eq!(K::Critical.label(), "CRITICAL");
        assert_eq!(K::Approval.label(), "APPROVAL");
    }

    /// C4: critical events and approvals reach the rail with tool names and summaries
    /// only; severity and DEFCON pushes are never recorded.
    #[test]
    fn approval_and_critical_events_become_rail_rows() {
        let id = Uuid::new_v4();
        let crit = rec(&ChatStreamEvent::CriticalEvent {
            turn_id: id,
            session_id: "s".into(),
            kind: "blocked".into(),
            summary: "terminal was blocked by rule a. It did not run.".into(),
        })
        .expect("a critical event is recorded");
        assert_eq!(crit.kind, ActivityEventKind::Critical);
        assert_eq!(
            crit.summary,
            "terminal was blocked by rule a. It did not run."
        );

        let requested = rec(&ChatStreamEvent::ApprovalRequested {
            turn_id: id,
            session_id: "s".into(),
            approval_id: "a".into(),
            tool: "terminal".into(),
            reason: "CANARY-REASON".into(),
        })
        .expect("an approval request is recorded");
        assert_eq!(requested.kind, ActivityEventKind::Approval);
        assert_eq!(requested.summary, "terminal requested");

        for (outcome, text) in [
            ("approved", "terminal approved"),
            ("denied", "terminal denied"),
            ("timed_out", "terminal timed out"),
            ("cancelled", "terminal cancelled"),
        ] {
            let r = rec(&ChatStreamEvent::ApprovalResolved {
                turn_id: id,
                session_id: "s".into(),
                approval_id: "a".into(),
                tool: "terminal".into(),
                outcome: outcome.into(),
            })
            .unwrap_or_else(|| panic!("{outcome} is recorded"));
            assert_eq!(r.kind, ActivityEventKind::Approval);
            assert_eq!(r.summary, text);
        }

        assert!(rec(&ChatStreamEvent::TurnSeverity {
            turn_id: id,
            session_id: "s".into(),
            worst: crate::protocol::TurnWorst::Red,
        })
        .is_none());
        assert!(rec(&ChatStreamEvent::DefconLevel {
            session_id: "s".into(),
            level: 4,
            bypass: true,
        })
        .is_none());
    }

    #[test]
    fn activity_log_now_hms_is_hh_mm_ss() {
        let s = now_hms();
        let parts: Vec<&str> = s.split(':').collect();
        assert_eq!(parts.len(), 3, "got {s}");
        assert!(parts.iter().all(|p| p.len() == 2 && p.chars().all(|c| c.is_ascii_digit())));
    }
}
