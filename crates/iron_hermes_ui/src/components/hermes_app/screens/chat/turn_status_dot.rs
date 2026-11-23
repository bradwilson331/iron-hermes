//! Phase 57 D-17 / UI-SPEC C1 — the per-turn status dot.
//!
//! Routine tool traffic in the web chat is one 12px dot per assistant turn instead of
//! one pane per call: a rotating hollow ring while the turn is active, a static solid
//! disc once it ends, tinted amber when a Warn ran, a bypass happened or an approval was
//! approved in that turn, and red on a Block or a tool failure. Severity only escalates
//! within a turn and each turn is independent.
//!
//! The dot never carries a tool name or argument (visible text, `title`, `aria-label`,
//! `data-*`): its label is one of four fixed strings. `TurnDotsCtx` is created and
//! provided only at the `HermesApp` root; this module consumes it, never provides it.
//! Severity is in-memory only — a bubble loaded from history or rebuilt after a refresh
//! has no entry and renders no dot (R14).

use crate::components::hermes_app::screens::chat::ChatBubble;
use crate::components::hermes_app::screens::chat::ChatBubbleKind;
use crate::protocol::{ChatStreamEvent, TurnWorst};
use crate::state::Screen;
use dioxus::prelude::*;
use std::collections::HashMap;

/// One turn's dot state: its worst severity so far and whether the turn is still running.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TurnDot {
    pub worst: TurnWorst,
    pub active: bool,
}

/// The client's per-turn severity map, keyed by the stream's `turn_id`.
pub type TurnDots = HashMap<uuid::Uuid, TurnDot>;

/// Context newtype for the per-turn severity signal, provided at the `HermesApp` root
/// (a provider declared in a child panics its consumers at runtime).
#[derive(Clone, Copy)]
pub struct TurnDotsCtx(pub Signal<TurnDots>);

/// Raise `turn_id`'s severity to at least `worst`; inserts an ended entry when absent.
fn raise(dots: &mut TurnDots, turn_id: uuid::Uuid, worst: TurnWorst) -> bool {
    match dots.get_mut(&turn_id) {
        Some(d) if d.worst >= worst => false,
        Some(d) => {
            d.worst = worst;
            true
        }
        None => {
            dots.insert(
                turn_id,
                TurnDot {
                    worst,
                    active: false,
                },
            );
            true
        }
    }
}

/// Mark `turn_id`'s turn as no longer running, when it is known.
fn end(dots: &mut TurnDots, turn_id: uuid::Uuid) -> bool {
    match dots.get_mut(&turn_id) {
        Some(d) if d.active => {
            d.active = false;
            true
        }
        _ => false,
    }
}

/// Fold one stream event into the per-turn map. Returns whether anything changed.
///
/// `TurnStarted` opens an active neutral entry; `TurnEnded`, `TurnCancelled` and (defensively)
/// `Finished` / `Error` close it; `TurnSeverity`, a `YoloBypassNotice`, an approved
/// `ApprovalResolved` (Amber) and a failed `ToolCallEnd` (Red) max-merge into it. A denied,
/// timed-out or cancelled approval changes nothing (the command did not run). A nil turn id
/// is ignored, and severity never drops.
pub fn fold_turn_event(dots: &mut TurnDots, ev: &ChatStreamEvent) -> bool {
    let nil = uuid::Uuid::nil();
    match ev {
        ChatStreamEvent::TurnStarted { turn_id, .. } if *turn_id != nil => {
            match dots.get_mut(turn_id) {
                Some(d) if d.active => false,
                Some(d) => {
                    d.active = true;
                    true
                }
                None => {
                    dots.insert(
                        *turn_id,
                        TurnDot {
                            worst: TurnWorst::Neutral,
                            active: true,
                        },
                    );
                    true
                }
            }
        }
        ChatStreamEvent::TurnEnded { turn_id, .. }
        | ChatStreamEvent::TurnCancelled { turn_id }
        | ChatStreamEvent::Finished { turn_id, .. }
        | ChatStreamEvent::Error { turn_id, .. }
            if *turn_id != nil =>
        {
            end(dots, *turn_id)
        }
        ChatStreamEvent::TurnSeverity { turn_id, worst, .. } if *turn_id != nil => {
            raise(dots, *turn_id, *worst)
        }
        ChatStreamEvent::YoloBypassNotice { turn_id, .. } if *turn_id != nil => {
            raise(dots, *turn_id, TurnWorst::Amber)
        }
        ChatStreamEvent::ApprovalResolved {
            turn_id, outcome, ..
        } if *turn_id != nil && outcome == "approved" => raise(dots, *turn_id, TurnWorst::Amber),
        ChatStreamEvent::ToolCallEnd {
            turn_id,
            success: false,
            ..
        } if *turn_id != nil => raise(dots, *turn_id, TurnWorst::Red),
        _ => false,
    }
}

/// The dot an assistant bubble renders: `Some` only for an Assistant bubble whose turn id
/// has an entry in the map (R14), so history and refreshed bubbles carry none.
pub fn dot_for_bubble(b: &ChatBubble, dots: &TurnDots) -> Option<TurnDot> {
    if b.kind != ChatBubbleKind::Assistant {
        return None;
    }
    dots.get(&b.turn_id?).copied()
}

/// The dot's accessible name: one of four fixed strings, never any tool data.
pub fn dot_label(worst: TurnWorst, active: bool) -> &'static str {
    match (worst, active) {
        (TurnWorst::Neutral, true) => "Turn status: working",
        (TurnWorst::Neutral, false) => "Turn status: finished",
        (TurnWorst::Amber, _) => "Turn status: warning",
        (TurnWorst::Red, _) => "Turn status: blocked or failed",
    }
}

/// The status dot: a native 24x24 button overlaid at the top-right of the assistant
/// bubble (12px visual drawn by CSS). Activating it switches to the Logs screen; it is not
/// a live region and renders no text.
#[component]
pub fn TurnStatusDot(worst: TurnWorst, active: bool) -> Element {
    let mut screen = use_context::<Signal<Screen>>();
    let tone = match worst {
        TurnWorst::Neutral => "is-neutral",
        TurnWorst::Amber => "is-amber",
        TurnWorst::Red => "is-red",
    };
    let class = if active {
        format!("ih-turn-dot {tone} is-spinning")
    } else {
        format!("ih-turn-dot {tone}")
    };
    rsx! {
        button {
            r#type: "button",
            class: "{class}",
            "aria-label": dot_label(worst, active),
            title: "See Logs for details",
            onclick: move |_| screen.set(Screen::Logs),
        }
    }
}

/// Every test in this plan builds a `ToolCallStart` through this helper, so a future change
/// to the wire variant touches one construction.
#[cfg(test)]
pub(crate) fn test_tool_call_start(turn_id: uuid::Uuid, name: &str) -> ChatStreamEvent {
    ChatStreamEvent::ToolCallStart {
        turn_id,
        name: name.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::hermes_app::screens::chat::{apply_transcript_event, ChatBubbleKind};

    /// A3 (T-57-58): tool arguments never reach the browser. `ToolCallStart` serializes
    /// with no `args` key, and the old wire form (with `args`) still deserializes.
    #[test]
    fn tool_call_start_has_no_args() {
        let ev = test_tool_call_start(uuid::Uuid::nil(), "terminal");
        let json = serde_json::to_value(&ev).unwrap();
        assert!(
            json["ToolCallStart"].get("args").is_none(),
            "no args key on the wire: {json}"
        );
        let old = r#"{"ToolCallStart":{"turn_id":"00000000-0000-0000-0000-000000000000","name":"terminal","args":"rm -rf /"}}"#;
        let back: ChatStreamEvent = serde_json::from_str(old).expect("old wire form deserializes");
        assert!(matches!(
            back,
            ChatStreamEvent::ToolCallStart { ref name, .. } if name == "terminal"
        ));
        assert!(!serde_json::to_string(&back).unwrap().contains("rm -rf"));
    }
    use std::cell::Cell;

    const CHAT_RS: &str = include_str!("../chat.rs");
    const MOD_RS: &str = include_str!("../../mod.rs");
    const SCREENS_CSS: &str = include_str!("../../../../../assets/screens.css");

    fn tid() -> uuid::Uuid {
        uuid::Uuid::new_v4()
    }

    fn started(t: uuid::Uuid) -> ChatStreamEvent {
        ChatStreamEvent::TurnStarted {
            turn_id: t,
            session_id: "s".into(),
            index: 0,
        }
    }

    fn ended(t: uuid::Uuid) -> ChatStreamEvent {
        ChatStreamEvent::TurnEnded {
            turn_id: t,
            session_id: "s".into(),
        }
    }

    fn tool_end(t: uuid::Uuid, success: bool) -> ChatStreamEvent {
        ChatStreamEvent::ToolCallEnd {
            turn_id: t,
            name: "terminal".into(),
            success,
        }
    }

    fn severity(t: uuid::Uuid, worst: TurnWorst) -> ChatStreamEvent {
        ChatStreamEvent::TurnSeverity {
            turn_id: t,
            session_id: "s".into(),
            worst,
        }
    }

    fn approval_resolved(t: uuid::Uuid, outcome: &str) -> ChatStreamEvent {
        ChatStreamEvent::ApprovalResolved {
            turn_id: t,
            session_id: "s".into(),
            approval_id: "a".into(),
            tool: "terminal".into(),
            outcome: outcome.into(),
        }
    }

    fn yolo(t: uuid::Uuid) -> ChatStreamEvent {
        ChatStreamEvent::YoloBypassNotice {
            turn_id: t,
            session_id: "s".into(),
            count: 2,
            tools: vec![("terminal".into(), 2)],
            reasons: vec![],
        }
    }

    /// Drive the same transcript reducer the HermesApp receive loop calls.
    fn run(events: &[ChatStreamEvent]) -> (Vec<ChatBubble>, TurnDots, Option<u64>) {
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
        (bubbles, dots, streaming)
    }

    thread_local! {
        static DOT_PROPS: Cell<(TurnWorst, bool)> = const { Cell::new((TurnWorst::Neutral, false)) };
    }

    fn render_dot(worst: TurnWorst, active: bool) -> String {
        DOT_PROPS.with(|p| p.set((worst, active)));
        fn root() -> Element {
            let screen = use_signal(|| Screen::Chat);
            use_context_provider(|| screen);
            let (worst, active) = DOT_PROPS.with(|p| p.get());
            rsx! { TurnStatusDot { worst, active } }
        }
        let mut dom = VirtualDom::new(root);
        dom.rebuild_in_place();
        dioxus::ssr::render(&dom)
    }

    #[test]
    fn dot_state_max_merges_per_turn() {
        let (t, u) = (tid(), tid());
        let mut d = TurnDots::new();
        assert!(fold_turn_event(&mut d, &started(t)));
        assert_eq!(
            d[&t],
            TurnDot {
                worst: TurnWorst::Neutral,
                active: true
            }
        );

        fold_turn_event(&mut d, &test_tool_call_start(t, "terminal"));
        fold_turn_event(&mut d, &tool_end(t, true));
        assert_eq!(
            d[&t].worst,
            TurnWorst::Neutral,
            "routine tool traffic stays neutral"
        );

        for amber in [
            yolo(t),
            approval_resolved(t, "approved"),
            severity(t, TurnWorst::Amber),
        ] {
            let mut d2 = TurnDots::new();
            fold_turn_event(&mut d2, &started(t));
            fold_turn_event(&mut d2, &amber);
            assert_eq!(d2[&t].worst, TurnWorst::Amber, "{amber:?}");
        }
        for red in [severity(t, TurnWorst::Red), tool_end(t, false)] {
            let mut d2 = TurnDots::new();
            fold_turn_event(&mut d2, &started(t));
            fold_turn_event(&mut d2, &red);
            assert_eq!(d2[&t].worst, TurnWorst::Red, "{red:?}");
        }

        fold_turn_event(&mut d, &severity(t, TurnWorst::Red));
        fold_turn_event(&mut d, &severity(t, TurnWorst::Amber));
        fold_turn_event(&mut d, &yolo(t));
        assert_eq!(
            d[&t].worst,
            TurnWorst::Red,
            "a later Amber never downgrades Red"
        );

        let mut d3 = TurnDots::new();
        fold_turn_event(&mut d3, &started(t));
        for outcome in ["denied", "timed_out", "cancelled"] {
            assert!(
                !fold_turn_event(&mut d3, &approval_resolved(t, outcome)),
                "{outcome}"
            );
        }
        assert_eq!(d3[&t].worst, TurnWorst::Neutral);

        let before = d.clone();
        fold_turn_event(&mut d, &severity(u, TurnWorst::Red));
        assert_eq!(d[&t], before[&t], "events for turn U leave T unchanged");
        assert_eq!(d[&u].worst, TurnWorst::Red);
        assert!(!fold_turn_event(
            &mut d,
            &severity(uuid::Uuid::nil(), TurnWorst::Red)
        ));
        assert!(
            !d.contains_key(&uuid::Uuid::nil()),
            "a nil turn id is ignored"
        );
    }

    #[test]
    fn routine_tool_call_renders_nothing_in_chat() {
        let t = tid();
        let (bubbles, dots, _) = run(&[
            started(t),
            test_tool_call_start(t, "terminal"),
            tool_end(t, true),
            ChatStreamEvent::Delta {
                turn_id: t,
                text: "done".into(),
            },
            ChatStreamEvent::Finished {
                turn_id: t,
                total_tokens: 1,
            },
            ended(t),
        ]);
        assert_eq!(bubbles.len(), 1, "{bubbles:?}");
        let b = &bubbles[0];
        assert_eq!(b.kind, ChatBubbleKind::Assistant);
        assert_eq!(b.turn_id, Some(t));
        assert_eq!(b.text, "done");
        assert!(
            !format!("{b:?}").contains("terminal"),
            "no tool data on the bubble: {b:?}"
        );
        assert_eq!(
            dot_for_bubble(b, &dots),
            Some(TurnDot {
                worst: TurnWorst::Neutral,
                active: false
            })
        );

        let html = render_dot(TurnWorst::Neutral, false);
        for needle in [
            "ih-turn-dot",
            "is-neutral",
            "type=\"button\"",
            "aria-label=\"Turn status: finished\"",
            "title=\"See Logs for details\"",
        ] {
            assert!(html.contains(needle), "missing `{needle}` in {html}");
        }
        assert!(!html.contains("terminal"));
    }

    #[test]
    fn dot_spins_only_while_turn_active() {
        let t = tid();
        let bubble_dot = |events: &[ChatStreamEvent]| {
            let (bubbles, dots, _) = run(events);
            dot_for_bubble(&bubbles[0], &dots).expect("a dot")
        };
        let delta = ChatStreamEvent::Delta {
            turn_id: t,
            text: "hi".into(),
        };
        assert!(bubble_dot(&[started(t), delta.clone()]).active);
        assert!(!bubble_dot(&[started(t), delta.clone(), ended(t)]).active);
        assert!(
            !bubble_dot(&[
                started(t),
                delta.clone(),
                ChatStreamEvent::TurnCancelled { turn_id: t }
            ])
            .active
        );
        assert!(
            !bubble_dot(&[
                started(t),
                delta.clone(),
                ChatStreamEvent::Finished {
                    turn_id: t,
                    total_tokens: 1
                }
            ])
            .active
        );

        let spinning = render_dot(TurnWorst::Neutral, true);
        assert!(spinning.contains("is-spinning") && spinning.contains("Turn status: working"));
        assert!(render_dot(TurnWorst::Amber, true).contains("Turn status: warning"));
        assert!(render_dot(TurnWorst::Red, false).contains("Turn status: blocked or failed"));
        assert!(!render_dot(TurnWorst::Neutral, false).contains("is-spinning"));
        assert!(!render_dot(TurnWorst::Red, false).contains("is-spinning"));
    }

    #[test]
    fn history_bubble_has_no_dot() {
        let t = tid();
        let mut dots = TurnDots::new();
        dots.insert(
            t,
            TurnDot {
                worst: TurnWorst::Neutral,
                active: false,
            },
        );
        assert_eq!(
            dot_for_bubble(&ChatBubble::assistant(1, "old".into()), &dots),
            None
        );
        assert_eq!(
            dot_for_bubble(&ChatBubble::assistant_for_turn(2, tid(), "x".into()), &dots),
            None,
            "turn id not in the map"
        );
        let mut user = ChatBubble::user(3, "q".into());
        user.turn_id = Some(t);
        assert_eq!(dot_for_bubble(&user, &dots), None, "non-assistant bubble");
        assert!(dot_for_bubble(&ChatBubble::assistant_for_turn(4, t, "x".into()), &dots).is_some());
    }

    /// The text between the `!matches!(` and the next `) {` of the bubble-wrap
    /// exclusion in chat.rs (the one that names `Divider`).
    fn bubble_wrap_exclusion(chat_rs: &str) -> String {
        let open = concat!("!matches", "!(");
        let mut from = 0;
        while let Some(i) = chat_rs[from..].find(open) {
            let start = from + i + open.len();
            if let Some(end) = chat_rs[start..].find(") {") {
                let body = &chat_rs[start..start + end];
                if body.contains("Divider") {
                    return body.to_string();
                }
            }
            from = start;
        }
        String::new()
    }

    #[test]
    fn transcript_render_wiring() {
        let exclusion = bubble_wrap_exclusion(CHAT_RS);
        assert!(
            exclusion.contains(concat!("ChatBubbleKind::", "Critical")),
            "the bubble-wrap exclusion must list Critical: `{exclusion}`"
        );
        for needle in [
            concat!("dot_for_bubble", "("),
            concat!("TurnStatusDot", " {"),
            concat!("CriticalRow", " {"),
            concat!("has-turn", "-dot"),
        ] {
            assert!(CHAT_RS.contains(needle), "chat.rs must contain `{needle}`");
        }
        for needle in [
            concat!("apply_transcript_event", "("),
            concat!("TurnDotsCtx", "("),
        ] {
            assert!(MOD_RS.contains(needle), "mod.rs must contain `{needle}`");
        }
    }

    /// Every `(selector, body)` block of a stylesheet, nested blocks included, comments removed.
    fn css_blocks(css: &str) -> Vec<(String, String)> {
        let mut clean = String::with_capacity(css.len());
        let mut rest = css;
        while let Some(i) = rest.find("/*") {
            clean.push_str(&rest[..i]);
            match rest[i..].find("*/") {
                Some(j) => rest = &rest[i + j + 2..],
                None => {
                    rest = "";
                }
            }
        }
        clean.push_str(rest);
        let mut out = Vec::new();
        let mut stack: Vec<(String, usize)> = Vec::new();
        let mut start = 0;
        for (i, c) in clean.char_indices() {
            match c {
                '{' => {
                    stack.push((clean[start..i].trim().to_string(), i + 1));
                    start = i + 1;
                }
                '}' => {
                    if let Some((sel, body_start)) = stack.pop() {
                        out.push((sel, clean[body_start..i].to_string()));
                    }
                    start = i + 1;
                }
                ';' => start = i + 1,
                _ => {}
            }
        }
        out
    }

    #[test]
    fn turn_dot_css_contract() {
        for needle in [
            ".ih-turn-dot",
            ".chat-bubble.has-turn-dot",
            "padding-right: 32px",
            "min-height: 32px",
            "width: 24px",
            "height: 24px",
            "position: absolute",
            "@keyframes rot",
            ".chat-critical-row",
        ] {
            assert!(
                SCREENS_CSS.contains(needle),
                "screens.css must contain `{needle}`"
            );
        }
        let blocks = css_blocks(SCREENS_CSS);
        assert!(
            blocks.iter().any(|(sel, body)| sel
                .starts_with("@media (prefers-reduced-motion: reduce)")
                && body.contains(".ih-turn-dot")),
            "a reduced-motion block must name .ih-turn-dot"
        );
        let summary = blocks
            .iter()
            .find(|(sel, _)| sel.contains(".chat-critical-summary"))
            .expect("a .chat-critical-summary rule");
        assert!(summary.1.contains("overflow-wrap: anywhere"), "{summary:?}");
        for (sel, body) in &blocks {
            if sel.contains("ih-turn-dot") || sel.contains("chat-critical") {
                assert!(!body.contains('#'), "hex literal in `{sel}`: {body}");
                for banned in ["--warn", "--danger", "--accent-primary"] {
                    assert!(!body.contains(banned), "`{banned}` in `{sel}`: {body}");
                }
            }
        }
    }
}
