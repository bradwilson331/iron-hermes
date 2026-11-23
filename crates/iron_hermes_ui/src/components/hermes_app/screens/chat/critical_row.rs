//! Phase 57 D-16 / UI-SPEC C2 — the critical transcript row.
//!
//! The chat transcript renders a row only for the five critical kinds (`blocked`,
//! `job_blocked`, `sidecar_unavailable`, `credential_detected`, `defcon_changed`). An
//! approval is rendered only by its card and a turn-ending tool failure only by the Error
//! bubble, so neither ever reaches this module. Severity colour is carried by the 2px
//! left stripe only; the label and summary stay in the text colours. The summary reaches
//! the DOM as an RSX text node, never as raw HTML (T-57-59).

use crate::components::hermes_app::screens::chat::{ChatBubble, ChatBubbleKind};
use dioxus::prelude::*;

/// The stripe colour of a critical row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CriticalTone {
    Red,
    Amber,
}

/// The label and stripe tone for a `CriticalEvent.kind`, or `None` for a kind that renders
/// no transcript row (the activity rail still records it).
pub fn critical_row_style(kind: &str) -> Option<(&'static str, CriticalTone)> {
    match kind {
        "blocked" => Some(("BLOCKED", CriticalTone::Red)),
        "job_blocked" => Some(("JOB BLOCKED", CriticalTone::Red)),
        // R5: always amber; a resulting block turns the turn's dot red through TurnSeverity.
        "sidecar_unavailable" => Some(("SECURITY CHECK UNAVAILABLE", CriticalTone::Amber)),
        "credential_detected" => Some(("CREDENTIAL DETECTED", CriticalTone::Amber)),
        "defcon_changed" => Some(("DEFCON CHANGED", CriticalTone::Amber)),
        _ => None,
    }
}

/// Push one critical row (R9a). When the LAST bubble is a Critical row with the same turn
/// id, kind and text, that row's `critical_repeat` is incremented instead of adding a
/// duplicate; any other bubble in between (or a different turn, kind or text, or a nil-turn
/// row against a turn row) keeps the rows separate, so a retry loop reads as one row with a
/// count while distinct events are never merged (T-57-78).
pub fn push_critical(
    bubbles: &mut Vec<ChatBubble>,
    next_id: &mut u64,
    turn_id: Option<uuid::Uuid>,
    kind: &str,
    summary: String,
) {
    if let Some(last) = bubbles.last_mut() {
        if last.kind == ChatBubbleKind::Critical
            && last.turn_id == turn_id
            && last.critical_kind.as_deref() == Some(kind)
            && last.text == summary
        {
            last.critical_repeat = last.critical_repeat.saturating_add(1);
            return;
        }
    }
    let id = *next_id;
    *next_id += 1;
    bubbles.push(ChatBubble::critical(id, turn_id, kind, summary));
}

/// One critical event as a no-avatar, full-width, non-interactive transcript row. A
/// `repeat` of 2 or more appends ` x{repeat}` to the summary (R9a).
#[component]
pub fn CriticalRow(kind: String, summary: String, repeat: u32) -> Element {
    let Some((label, tone)) = critical_row_style(&kind) else {
        return rsx! {};
    };
    let class = match tone {
        CriticalTone::Red => "chat-critical-row is-red",
        CriticalTone::Amber => "chat-critical-row is-amber",
    };
    // One text node, so the count is never split from the summary it qualifies.
    let text = if repeat >= 2 {
        format!("{summary} x{repeat}")
    } else {
        summary
    };
    rsx! {
        div { class: "{class}", role: "status", "aria-live": "polite",
            span { class: "chat-critical-label", "{label}" }
            span { class: "chat-critical-summary", "{text}" }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::hermes_app::screens::chat::turn_status_dot::{
        dot_for_bubble, test_tool_call_start, TurnDot, TurnDots,
    };
    use crate::components::hermes_app::screens::chat::{
        apply_transcript_event, ChatBubble, ChatBubbleKind,
    };
    use crate::protocol::{ChatStreamEvent, TurnWorst};
    use std::cell::RefCell;

    fn tid() -> uuid::Uuid {
        uuid::Uuid::new_v4()
    }

    fn run(events: &[ChatStreamEvent]) -> (Vec<ChatBubble>, TurnDots) {
        let mut bubbles = Vec::new();
        let mut next_id = 1u64;
        let mut streaming = None;
        let mut dots = TurnDots::new();
        for ev in events {
            apply_transcript_event(
                &mut bubbles,
                &mut next_id,
                &mut streaming,
                &mut dots,
                "s",
                ev,
            );
        }
        (bubbles, dots)
    }

    fn started(t: uuid::Uuid) -> ChatStreamEvent {
        ChatStreamEvent::TurnStarted {
            turn_id: t,
            session_id: "s".into(),
            index: 0,
        }
    }

    fn critical(t: uuid::Uuid, kind: &str, summary: &str) -> ChatStreamEvent {
        ChatStreamEvent::CriticalEvent {
            turn_id: t,
            session_id: "s".into(),
            kind: kind.into(),
            summary: summary.into(),
        }
    }

    thread_local! {
        static ROW_PROPS: RefCell<(String, String, u32)> =
            const { RefCell::new((String::new(), String::new(), 1)) };
    }

    fn render_row(kind: &str, summary: &str, repeat: u32) -> String {
        ROW_PROPS.with(|p| *p.borrow_mut() = (kind.to_string(), summary.to_string(), repeat));
        fn root() -> Element {
            let (kind, summary, repeat) = ROW_PROPS.with(|p| p.borrow().clone());
            rsx! { CriticalRow { kind, summary, repeat } }
        }
        let mut dom = VirtualDom::new(root);
        dom.rebuild_in_place();
        dioxus::ssr::render(&dom)
    }

    #[test]
    fn blocked_turn_renders_red_dot_and_one_critical_row() {
        let t = tid();
        let s = "terminal was blocked by rule guardrail.rm_rf. It did not run.";
        let (bubbles, dots) = run(&[
            started(t),
            test_tool_call_start(t, "terminal"),
            ChatStreamEvent::TurnSeverity {
                turn_id: t,
                session_id: "s".into(),
                worst: TurnWorst::Red,
            },
            critical(t, "blocked", s),
        ]);
        let crit: Vec<_> = bubbles
            .iter()
            .filter(|b| b.kind == ChatBubbleKind::Critical)
            .collect();
        assert_eq!(crit.len(), 1, "{bubbles:?}");
        assert_eq!(crit[0].critical_kind.as_deref(), Some("blocked"));
        assert_eq!(crit[0].text, s);
        assert_eq!(
            bubbles[0].kind,
            ChatBubbleKind::Assistant,
            "the assistant bubble comes first"
        );
        assert_eq!(
            dot_for_bubble(&bubbles[0], &dots),
            Some(TurnDot {
                worst: TurnWorst::Red,
                active: true
            })
        );

        let html = render_row("blocked", s, 1);
        for needle in [
            "chat-critical-row",
            "is-red",
            "BLOCKED",
            s,
            "role=\"status\"",
            "aria-live=\"polite\"",
        ] {
            assert!(html.contains(needle), "missing `{needle}` in {html}");
        }

        assert_eq!(
            critical_row_style("blocked").map(|x| x.1),
            Some(CriticalTone::Red)
        );
        assert_eq!(
            critical_row_style("job_blocked").map(|x| x.1),
            Some(CriticalTone::Red)
        );
        for k in [
            "sidecar_unavailable",
            "credential_detected",
            "defcon_changed",
        ] {
            assert_eq!(
                critical_row_style(k).map(|x| x.1),
                Some(CriticalTone::Amber),
                "{k}"
            );
        }
        assert_eq!(critical_row_style("approval_requested"), None);
        assert_eq!(critical_row_style("nonsense"), None);
    }

    #[test]
    fn critical_row_text_is_not_html() {
        let html = render_row("blocked", "<img src=x onerror=alert(1)>", 1);
        // The SSR escapes `<` as the numeric entity `&#60;` (a browser shows it as text).
        assert!(
            html.contains("&#60;img") || html.contains("&lt;img"),
            "the markup must arrive escaped: {html}"
        );
        assert!(!html.contains("<img src=x"), "{html}");
    }

    #[test]
    fn approval_and_turn_failure_render_once() {
        let t = tid();
        let (bubbles, _) = run(&[
            started(t),
            ChatStreamEvent::Error {
                turn_id: t,
                message: "boom".into(),
            },
        ]);
        assert_eq!(
            bubbles
                .iter()
                .filter(|b| b.kind == ChatBubbleKind::Error)
                .count(),
            1
        );
        assert_eq!(
            bubbles
                .iter()
                .filter(|b| b.kind == ChatBubbleKind::Critical)
                .count(),
            0
        );

        let (bubbles, _) = run(&[
            started(t),
            ChatStreamEvent::ApprovalRequested {
                turn_id: t,
                session_id: "s".into(),
                approval_id: "a".into(),
                tool: "terminal".into(),
                reason: "r".into(),
            },
            ChatStreamEvent::ApprovalResolved {
                turn_id: t,
                session_id: "s".into(),
                approval_id: "a".into(),
                tool: "terminal".into(),
                outcome: "approved".into(),
            },
        ]);
        assert_eq!(
            bubbles
                .iter()
                .filter(|b| b.kind == ChatBubbleKind::Critical)
                .count(),
            0
        );
    }

    fn yolo_notice(t: uuid::Uuid) -> ChatStreamEvent {
        ChatStreamEvent::YoloBypassNotice {
            turn_id: t,
            session_id: "s".into(),
            count: 2,
            tools: vec![("terminal".into(), 2)],
            reasons: vec![],
        }
    }

    /// D-19: a bypass notice turns the turn's dot amber and adds no row or bubble; the
    /// 54.3 chip is gone, so nothing else consumes the notice on the client.
    #[test]
    fn yolo_notice_turns_dot_amber_no_chip() {
        let t = tid();
        let (before, _) = run(&[started(t), test_tool_call_start(t, "terminal")]);
        let (after, dots) = run(&[
            started(t),
            test_tool_call_start(t, "terminal"),
            yolo_notice(t),
        ]);
        assert_eq!(before.len(), after.len(), "the notice adds no bubble");
        assert_eq!(
            dot_for_bubble(&after[0], &dots),
            Some(TurnDot {
                worst: TurnWorst::Amber,
                active: true
            })
        );
    }

    /// The text of `src` with `//` line comments removed (code only).
    fn rust_code(src: &str) -> String {
        src.lines()
            .map(|l| l.split("//").next().unwrap_or(""))
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// The text of a stylesheet with `/* */` comments removed.
    fn css_code(css: &str) -> String {
        let mut out = String::new();
        let mut rest = css;
        while let Some(i) = rest.find("/*") {
            out.push_str(&rest[..i]);
            rest = match rest[i..].find("*/") {
                Some(j) => &rest[i + j + 2..],
                None => "",
            };
        }
        out.push_str(rest);
        out
    }

    /// R11/R12: the superseded 54.3 chip and 49.4 tool-pane code is deleted, not left as dead
    /// code. Needles are built with `concat!` so this file never matches itself.
    #[test]
    fn superseded_transcript_code_removed() {
        const CHAT_RS: &str = include_str!("../chat.rs");
        const MOD_RS: &str = include_str!("../../mod.rs");
        const STATE_RS: &str = include_str!("../../../../state.rs");
        const SCREENS_CSS: &str = include_str!("../../../../../assets/screens.css");

        let chat = rust_code(CHAT_RS);
        for needle in [
            concat!("Tool", "Row"),
            concat!("tool", "_rows"),
            concat!("group_tool", "_rows"),
            concat!("TOOL_GROUP", "_THRESHOLD"),
            concat!("Tool", "Group"),
            concat!("render_tool", "_pane"),
            concat!("render_tool", "_group"),
            concat!("auto_follow_running", "_tool_panes"),
            concat!("auto_follow_scrollable", "_progress_tail"),
            concat!("on_scrollable", "_progress_scroll"),
            concat!("tool_pane", "_expanded"),
            concat!("tool_pane", "_scrolled_away"),
            concat!("YoloIndicator", "Ctx"),
            concat!("yolo", "-bypass"),
        ] {
            assert!(!chat.contains(needle), "chat.rs still contains `{needle}`");
        }

        let app = rust_code(MOD_RS);
        for needle in [
            concat!("YoloIndicator", "State"),
            concat!("YoloIndicator", "Ctx"),
            concat!("yolo", "_indicator"),
            concat!("tool", "_rows"),
            concat!("Tool", "Row"),
        ] {
            assert!(!app.contains(needle), "mod.rs still contains `{needle}`");
        }

        let state = rust_code(STATE_RS);
        for needle in [
            concat!("YoloIndicator", "State"),
            concat!("YoloIndicator", "Ctx"),
            concat!("YOLO_INDICATOR", "_MAX"),
        ] {
            assert!(
                !state.contains(needle),
                "state.rs still contains `{needle}`"
            );
        }

        let css = css_code(SCREENS_CSS);
        assert!(
            !css.contains(concat!(".chat", "-progress")),
            "screens.css still has a progress rule"
        );
        assert!(
            !css.contains(concat!(".yolo", "-bypass")),
            "screens.css still has a yolo-bypass rule"
        );
        assert!(
            css.contains(concat!("@keyframes", " rot")),
            "the dot still needs the rot keyframe"
        );
    }

    fn crit_count(bubbles: &[ChatBubble]) -> usize {
        bubbles
            .iter()
            .filter(|b| b.kind == ChatBubbleKind::Critical)
            .count()
    }

    /// R9a: consecutive identical critical rows coalesce into one with a repeat count.
    #[test]
    fn critical_rows_coalesce_consecutive_identical() {
        let t = tid();
        let s = "terminal was blocked by rule a. It did not run.";
        let mut bubbles = Vec::new();
        let mut next_id = 1u64;
        push_critical(&mut bubbles, &mut next_id, Some(t), "blocked", s.into());
        push_critical(&mut bubbles, &mut next_id, Some(t), "blocked", s.into());
        assert_eq!(crit_count(&bubbles), 1);
        assert_eq!(bubbles[0].critical_repeat, 2);
        push_critical(&mut bubbles, &mut next_id, Some(t), "blocked", s.into());
        assert_eq!(crit_count(&bubbles), 1);
        assert_eq!(bubbles[0].critical_repeat, 3);

        let html = render_row("blocked", s, 3);
        assert!(html.contains(&format!("{s} x3")), "{html}");
        assert!(
            !render_row("blocked", s, 1).contains(" x1"),
            "no suffix at repeat 1"
        );
        assert!(
            !render_row("blocked", s, 2).contains(" x1"),
            "repeat 2 reads x2"
        );
        assert!(render_row("blocked", s, 2).contains(" x2"));
    }

    /// R9a: a different summary, kind or turn, an interleaved bubble, or a nil-turn row
    /// keeps the rows separate.
    #[test]
    fn critical_rows_distinct_or_interleaved_stay_separate() {
        let t = tid();
        let u = tid();
        let push2 = |a: (Option<uuid::Uuid>, &str, &str), b: (Option<uuid::Uuid>, &str, &str)| {
            let mut bubbles = Vec::new();
            let mut next_id = 1u64;
            push_critical(&mut bubbles, &mut next_id, a.0, a.1, a.2.into());
            push_critical(&mut bubbles, &mut next_id, b.0, b.1, b.2.into());
            bubbles
        };
        let base = (Some(t), "blocked", "x was blocked.");
        assert_eq!(
            crit_count(&push2(base, (Some(t), "blocked", "y was blocked."))),
            2
        );
        assert_eq!(
            crit_count(&push2(base, (Some(t), "defcon_changed", "x was blocked."))),
            2
        );
        assert_eq!(
            crit_count(&push2(base, (Some(u), "blocked", "x was blocked."))),
            2
        );
        assert_eq!(
            crit_count(&push2(base, (None, "blocked", "x was blocked."))),
            2,
            "a nil-turn row never coalesces with a row of turn T"
        );

        let mut bubbles = Vec::new();
        let mut next_id = 1u64;
        push_critical(&mut bubbles, &mut next_id, Some(t), "blocked", "x".into());
        bubbles.push(ChatBubble::error(99, "boom".into()));
        push_critical(&mut bubbles, &mut next_id, Some(t), "blocked", "x".into());
        assert_eq!(crit_count(&bubbles), 2, "an interleaved bubble separates");
        assert!(bubbles
            .iter()
            .filter(|b| b.kind == ChatBubbleKind::Critical)
            .all(|b| b.critical_repeat == 1));
    }
}
