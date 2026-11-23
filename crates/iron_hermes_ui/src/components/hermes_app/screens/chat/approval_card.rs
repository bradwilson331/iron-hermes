//! Phase 57 D-14 / UI-SPEC C3 — the Approve/Deny card for a parked web approval.
//!
//! A web Warn parks in the server's `WebApprovalRegistry` until an authenticated decision, a
//! timeout or a cancel. The server emits `ApprovalRequested`; this module turns it into one
//! transcript card keyed by `approval_id`, whose buttons call the authenticated
//! `resolve_web_approval` server function. The card never takes focus (a stray Enter or
//! Space typed in the composer must not approve a command), keeps its place in the
//! transcript after resolution as the record, and shows only the tool name and the
//! redacted reason as RSX text nodes (T-57-80).
//!
//! The state machine is the pure `reduce_card`; `apply_approval_event` is the receive loop's
//! reducer for the two approval wire events. `ApprovalCardsCtx` is created and provided only
//! at the `HermesApp` root; this module consumes it, never provides it.

use crate::components::hermes_app::screens::chat::ChatBubble;
use crate::protocol::ChatStreamEvent;
use dioxus::prelude::*;
use std::collections::HashMap;

/// How a card ended: the four outcomes the server reports.
#[derive(Clone, Debug, PartialEq)]
pub enum ApprovalCardOutcome {
    Approved,
    Denied,
    TimedOut,
    Cancelled,
    /// The server fn answered `Ok(false)`: the approval was already answered, timed out, or
    /// its turn ended, so nothing was changed. Not a wire outcome (`from_wire` never gives it).
    Expired,
}

impl ApprovalCardOutcome {
    /// The wire `outcome` of an `ApprovalResolved` event, or `None` for an unknown word.
    pub fn from_wire(outcome: &str) -> Option<Self> {
        match outcome {
            "approved" => Some(Self::Approved),
            "denied" => Some(Self::Denied),
            "timed_out" => Some(Self::TimedOut),
            "cancelled" => Some(Self::Cancelled),
            _ => None,
        }
    }

    /// The uppercase outcome label shown in place of the buttons.
    pub fn label(&self) -> &'static str {
        match self {
            Self::Approved => "APPROVED",
            Self::Denied => "DENIED",
            Self::TimedOut => "TIMED OUT - DENIED",
            Self::Cancelled => "CANCELLED - THE TURN ENDED",
            Self::Expired => "NO LONGER PENDING",
        }
    }

    /// The stripe tone class: green for approved, amber for a timeout, dim otherwise.
    pub fn tone(&self) -> &'static str {
        match self {
            Self::Approved => "is-green",
            Self::Denied | Self::Cancelled | Self::Expired => "is-dim",
            Self::TimedOut => "is-amber",
        }
    }

    /// The detail line shown under the label, for the outcomes that need one.
    pub fn detail(&self) -> Option<&'static str> {
        match self {
            Self::Expired => Some(
                "This approval was already answered, timed out, or its turn ended. Nothing was changed.",
            ),
            _ => None,
        }
    }
}

/// The line shown under the buttons after a failed call or a dropped socket.
const ERROR_COPY: &str =
    "Could not reach the server. The request will time out and be denied. You can try again.";

/// Where one card is in its life.
#[derive(Clone, Debug, PartialEq)]
pub enum ApprovalCardState {
    /// Waiting on the operator; `error` is set when a call failed and the buttons are back.
    Pending { error: bool },
    /// A decision is in flight to the server.
    Submitted { approved: bool },
    /// The approval is over; the card stays as the record.
    Resolved { outcome: ApprovalCardOutcome },
}

/// One input to a card's state machine.
#[derive(Clone, Debug, PartialEq)]
pub enum CardAction {
    /// The operator pressed Approve or Deny.
    Click { approved: bool },
    /// `resolve_web_approval` returned `Ok(resolved)` for the decision `approved`.
    ServerReply { approved: bool, resolved: bool },
    /// An `ApprovalResolved` event arrived with this wire outcome.
    Resolved(String),
    /// `resolve_web_approval` failed (`Err`): the request did not reach the server.
    ServerErr,
    /// The chat socket dropped, so the server-side state of this card is no longer known.
    Disconnected,
}

/// The next state of a card. Pure: no signal, no I/O.
///
/// A click on a Pending card submits it. `Ok(true)` from the server fn means the server
/// accepted exactly that decision, so the card resolves from the reply and never depends on
/// an `ApprovalResolved` event a dropped socket could lose. A known `Resolved` outcome from
/// the server wins over the optimistic Submitted state. A Resolved card is terminal, except
/// that the server's real outcome is more informative than `Expired`, so it replaces it.
///
/// `Ok(false)` ends a card as `Expired` (nothing was changed). An `Err` or a dropped socket
/// returns a Submitted or Pending card to `Pending { error: true }` with its buttons back, so
/// the card never stays disabled: a reply still in flight, an event, or a new click moves it
/// on from there.
pub fn reduce_card(state: &ApprovalCardState, action: CardAction) -> ApprovalCardState {
    match (state, action) {
        (
            ApprovalCardState::Resolved {
                outcome: ApprovalCardOutcome::Expired,
            },
            CardAction::Resolved(wire),
        ) => match ApprovalCardOutcome::from_wire(&wire) {
            Some(outcome) => ApprovalCardState::Resolved { outcome },
            None => state.clone(),
        },
        (ApprovalCardState::Resolved { .. }, _) => state.clone(),
        (_, CardAction::Resolved(wire)) => match ApprovalCardOutcome::from_wire(&wire) {
            Some(outcome) => ApprovalCardState::Resolved { outcome },
            None => state.clone(),
        },
        (
            ApprovalCardState::Pending { .. } | ApprovalCardState::Submitted { .. },
            CardAction::ServerReply {
                approved,
                resolved: true,
            },
        ) => ApprovalCardState::Resolved {
            outcome: if approved {
                ApprovalCardOutcome::Approved
            } else {
                ApprovalCardOutcome::Denied
            },
        },
        (
            ApprovalCardState::Pending { .. } | ApprovalCardState::Submitted { .. },
            CardAction::ServerReply {
                resolved: false, ..
            },
        ) => ApprovalCardState::Resolved {
            outcome: ApprovalCardOutcome::Expired,
        },
        (ApprovalCardState::Pending { .. }, CardAction::Click { approved }) => {
            ApprovalCardState::Submitted { approved }
        }
        (ApprovalCardState::Submitted { .. }, CardAction::ServerErr)
        | (
            ApprovalCardState::Pending { .. } | ApprovalCardState::Submitted { .. },
            CardAction::Disconnected,
        ) => ApprovalCardState::Pending { error: true },
        _ => state.clone(),
    }
}

/// What the transcript needs to render one card.
#[derive(Clone, Debug, PartialEq)]
pub struct ApprovalCardModel {
    pub session_id: String,
    pub tool: String,
    pub reason: String,
    pub state: ApprovalCardState,
}

/// Every card this page has seen, keyed by `approval_id`.
pub type ApprovalCards = HashMap<String, ApprovalCardModel>;

/// Context newtype for the card map, provided at the `HermesApp` root (a provider declared
/// in a child panics its consumers at runtime).
#[derive(Clone, Copy)]
pub struct ApprovalCardsCtx(pub Signal<ApprovalCards>);

/// The socket dropped: every open card returns to Pending with the error line. Returns
/// whether anything changed.
pub fn apply_disconnect(cards: &mut ApprovalCards) -> bool {
    disconnect_matching(cards, |_| true)
}

/// RR-03: a drop of the OpenUI socket only says something about the approvals its own turns
/// raised. Disconnects the cards of `session_id` and leaves every other session's cards (Chat's)
/// alone. Returns whether anything changed.
pub fn apply_disconnect_for_session(cards: &mut ApprovalCards, session_id: &str) -> bool {
    disconnect_matching(cards, |m| m.session_id == session_id)
}

fn disconnect_matching(
    cards: &mut ApprovalCards,
    mut pick: impl FnMut(&ApprovalCardModel) -> bool,
) -> bool {
    let mut changed = false;
    for model in cards.values_mut().filter(|m| pick(m)) {
        let next = reduce_card(&model.state, CardAction::Disconnected);
        if next != model.state {
            model.state = next;
            changed = true;
        }
    }
    changed
}

/// Whether any card is waiting on the operator (Pending without an error) or has a decision
/// in flight (Submitted). Drives the Chat nav badge.
///
/// A `Pending { error: true }` card's server state is unknown after a drop or a failed call,
/// so it shows its error line and retry buttons in the transcript but does not light the
/// badge. Every badge-lighting state has an exit: a reply, an event, a click, or a drop.
pub fn any_approval_pending(cards: &ApprovalCards) -> bool {
    cards.values().any(|m| {
        matches!(
            m.state,
            ApprovalCardState::Pending { error: false } | ApprovalCardState::Submitted { .. }
        )
    })
}

/// Fold one approval wire event into the transcript and the card map. Returns whether
/// anything changed.
///
/// `ApprovalRequested` for an id not yet seen inserts a Pending model and pushes the card's
/// transcript slot; a repeat for a known id does nothing, so a session never shows two cards
/// for one approval. `ApprovalResolved` for a known id applies the outcome; an unknown id
/// does nothing. The card is never a critical row (R4): the approval has exactly one surface.
pub fn apply_approval_event(
    bubbles: &mut Vec<ChatBubble>,
    next_id: &mut u64,
    cards: &mut ApprovalCards,
    ev: &ChatStreamEvent,
) -> bool {
    match ev {
        ChatStreamEvent::ApprovalRequested {
            session_id,
            approval_id,
            tool,
            reason,
            ..
        } => {
            if cards.contains_key(approval_id) {
                return false;
            }
            cards.insert(
                approval_id.clone(),
                ApprovalCardModel {
                    session_id: session_id.clone(),
                    tool: tool.clone(),
                    reason: reason.clone(),
                    state: ApprovalCardState::Pending { error: false },
                },
            );
            let id = *next_id;
            *next_id += 1;
            bubbles.push(ChatBubble::approval(id, approval_id));
            true
        }
        ChatStreamEvent::ApprovalResolved {
            approval_id,
            outcome,
            ..
        } => match cards.get_mut(approval_id) {
            Some(model) => {
                let next = reduce_card(&model.state, CardAction::Resolved(outcome.clone()));
                if next == model.state {
                    false
                } else {
                    model.state = next;
                    true
                }
            }
            None => false,
        },
        _ => false,
    }
}

/// Apply `action` to the card `approval_id` in the shared map, writing the signal only when
/// the state changed. Returns the state after, or `None` for an unknown id. The signal is
/// read into an owned value first, so no borrow is held while the state is computed.
fn apply_card_action(
    cards: &mut Signal<ApprovalCards>,
    approval_id: &str,
    action: CardAction,
) -> Option<ApprovalCardState> {
    let before = cards.read().get(approval_id).map(|m| m.state.clone())?;
    let after = reduce_card(&before, action);
    if after != before {
        cards.with_mut(|map| {
            if let Some(model) = map.get_mut(approval_id) {
                model.state = after.clone();
            }
        });
    }
    Some(after)
}

/// The operator pressed Approve or Deny on `approval_id`: move the card to Submitted, then
/// call the authenticated server fn and fold its reply into the card. Nothing is sent for a
/// card that did not accept the click (already Submitted or Resolved). No signal borrow is
/// held across the await.
fn submit_decision(mut cards: Signal<ApprovalCards>, approval_id: String, approved: bool) {
    let Some(after) = apply_card_action(&mut cards, &approval_id, CardAction::Click { approved })
    else {
        return;
    };
    if !matches!(after, ApprovalCardState::Submitted { .. }) {
        return;
    }
    spawn(async move {
        let reply =
            crate::server::approval_api::resolve_web_approval(approval_id.clone(), approved).await;
        let action = match reply {
            Ok(resolved) => CardAction::ServerReply { approved, resolved },
            Err(_) => CardAction::ServerErr,
        };
        apply_card_action(&mut cards, &approval_id, action);
    });
}

/// One parked approval as a no-avatar, full-width transcript card.
///
/// The card is a live region (`role="alert"` while Pending, `role="status"` afterwards) and
/// never moves focus. The tool name and the redacted reason are text nodes. The Approve and
/// Deny buttons are native buttons; Deny is the neutral secondary style, never red.
#[component]
pub fn ApprovalCard(
    approval_id: String,
    tool: String,
    reason: String,
    state: ApprovalCardState,
) -> Element {
    let cards = use_context::<ApprovalCardsCtx>().0;
    let pending = matches!(state, ApprovalCardState::Pending { .. });
    let submitted = matches!(state, ApprovalCardState::Submitted { .. });
    let tone = match &state {
        ApprovalCardState::Pending { .. } | ApprovalCardState::Submitted { .. } => "is-amber",
        ApprovalCardState::Resolved { outcome } => outcome.tone(),
    };
    let outcome_label = match &state {
        ApprovalCardState::Resolved { outcome } => Some(outcome.label()),
        _ => None,
    };
    let outcome_detail = match &state {
        ApprovalCardState::Resolved { outcome } => outcome.detail(),
        _ => None,
    };
    let show_error = matches!(state, ApprovalCardState::Pending { error: true });
    let class = format!("chat-approval-card {tone}");
    let role = if pending { "alert" } else { "status" };
    let show_actions = pending || submitted;
    let approve_id = approval_id.clone();
    let deny_id = approval_id;
    let approve_label = format!("Approve {tool}");
    let deny_label = format!("Deny {tool}");
    rsx! {
        div { class: "{class}", role: "{role}",
            div { class: "chat-approval-title",
                span { class: "chat-approval-label", "APPROVAL NEEDED" }
                code { "{tool}" }
            }
            if !reason.is_empty() {
                div { class: "chat-approval-reason", "{reason}" }
            }
            if pending {
                div { class: "chat-approval-helper", "No answer is treated as Deny." }
            }
            if show_actions {
                div { class: "chat-approval-actions",
                    button {
                        r#type: "button",
                        class: "ih-btn-primary",
                        "aria-label": "{approve_label}",
                        disabled: submitted,
                        onclick: move |_| submit_decision(cards, approve_id.clone(), true),
                        "Approve"
                    }
                    button {
                        r#type: "button",
                        class: "ih-btn-secondary",
                        "aria-label": "{deny_label}",
                        disabled: submitted,
                        onclick: move |_| submit_decision(cards, deny_id.clone(), false),
                        "Deny"
                    }
                }
            }
            if show_error {
                div { class: "chat-approval-error", "{ERROR_COPY}" }
            }
            if submitted {
                div { class: "chat-approval-status", "Sending your decision..." }
            }
            if let Some(label) = outcome_label {
                span { class: "chat-approval-outcome", "{label}" }
            }
            if let Some(detail) = outcome_detail {
                div { class: "chat-approval-status", "{detail}" }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::hermes_app::screens::chat::{ChatBubble, ChatBubbleKind};
    use crate::protocol::ChatStreamEvent;
    use std::cell::RefCell;

    fn pending() -> ApprovalCardState {
        ApprovalCardState::Pending { error: false }
    }

    thread_local! {
        static CARD_PROPS: RefCell<(String, String, String, ApprovalCardState)> =
            const { RefCell::new((
                String::new(),
                String::new(),
                String::new(),
                ApprovalCardState::Pending { error: false },
            )) };
    }

    fn render_card(tool: &str, reason: &str, state: ApprovalCardState) -> String {
        CARD_PROPS.with(|p| {
            *p.borrow_mut() = (
                "a1".to_string(),
                tool.to_string(),
                reason.to_string(),
                state,
            )
        });
        fn root() -> Element {
            let cards = use_signal(ApprovalCards::default);
            use_context_provider(|| ApprovalCardsCtx(cards));
            let screen = use_signal(|| crate::state::Screen::Chat);
            use_context_provider(|| screen);
            let (approval_id, tool, reason, state) = CARD_PROPS.with(|p| p.borrow().clone());
            rsx! { ApprovalCard { approval_id, tool, reason, state } }
        }
        let mut dom = VirtualDom::new(root);
        dom.rebuild_in_place();
        dioxus::ssr::render(&dom)
    }

    fn requested(approval_id: &str, tool: &str) -> ChatStreamEvent {
        ChatStreamEvent::ApprovalRequested {
            turn_id: uuid::Uuid::new_v4(),
            session_id: "s".into(),
            approval_id: approval_id.into(),
            tool: tool.into(),
            reason: "curl to example.com".into(),
        }
    }

    fn resolved(approval_id: &str, outcome: &str) -> ChatStreamEvent {
        ChatStreamEvent::ApprovalResolved {
            turn_id: uuid::Uuid::new_v4(),
            session_id: "s".into(),
            approval_id: approval_id.into(),
            tool: "terminal".into(),
            outcome: outcome.into(),
        }
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

    #[test]
    fn approval_card_renders_tool_and_reason() {
        let html = render_card("terminal", "curl to example.com", pending());
        for needle in [
            "chat-approval-card",
            "is-amber",
            "APPROVAL NEEDED",
            "<code>terminal</code>",
            "curl to example.com",
            "No answer is treated as Deny.",
            "role=\"alert\"",
            "aria-label=\"Approve terminal\"",
            "aria-label=\"Deny terminal\"",
            "ih-btn-primary",
            "ih-btn-secondary",
        ] {
            assert!(html.contains(needle), "missing `{needle}` in {html}");
        }
        assert_eq!(
            html.matches("type=\"button\"").count(),
            2,
            "both buttons are native type=button: {html}"
        );
        assert!(
            html.contains(">Approve<") && html.contains(">Deny<"),
            "{html}"
        );
        assert!(!html.contains("autofocus"), "no autofocus: {html}");
        assert!(
            !html.contains("disabled"),
            "Pending buttons are enabled: {html}"
        );
    }

    #[test]
    fn approval_card_text_is_not_html() {
        let html = render_card("terminal", "<img src=x onerror=alert(1)>", pending());
        // The SSR escapes `<` as the numeric entity `&#60;` (a browser shows it as text).
        assert!(
            html.contains("&#60;img") || html.contains("&lt;img"),
            "the markup must arrive escaped: {html}"
        );
        assert!(!html.contains("<img src=x"), "{html}");
        let html = render_card("<b>x</b>", "r", pending());
        assert!(
            !html.contains("<b>x</b>"),
            "the tool name is a text node: {html}"
        );
    }

    #[test]
    fn approval_card_disables_after_click() {
        assert_eq!(
            reduce_card(&pending(), CardAction::Click { approved: true }),
            ApprovalCardState::Submitted { approved: true }
        );
        let html = render_card(
            "terminal",
            "r",
            ApprovalCardState::Submitted { approved: true },
        );
        assert!(html.contains("Sending your decision..."), "{html}");
        assert!(html.contains("role=\"status\""), "{html}");
        assert!(!html.contains("role=\"alert\""), "{html}");
        assert_eq!(
            html.matches("disabled").count(),
            2,
            "both buttons carry the disabled attribute: {html}"
        );
        assert_eq!(
            reduce_card(
                &ApprovalCardState::Submitted { approved: true },
                CardAction::ServerReply {
                    approved: true,
                    resolved: true
                }
            ),
            ApprovalCardState::Resolved {
                outcome: ApprovalCardOutcome::Approved
            }
        );
        assert_eq!(
            reduce_card(
                &ApprovalCardState::Submitted { approved: false },
                CardAction::ServerReply {
                    approved: false,
                    resolved: true
                }
            ),
            ApprovalCardState::Resolved {
                outcome: ApprovalCardOutcome::Denied
            }
        );
    }

    #[test]
    fn approval_card_shows_outcome() {
        for (wire, label, tone, outcome) in [
            (
                "approved",
                "APPROVED",
                "is-green",
                ApprovalCardOutcome::Approved,
            ),
            ("denied", "DENIED", "is-dim", ApprovalCardOutcome::Denied),
            (
                "timed_out",
                "TIMED OUT - DENIED",
                "is-amber",
                ApprovalCardOutcome::TimedOut,
            ),
            (
                "cancelled",
                "CANCELLED - THE TURN ENDED",
                "is-dim",
                ApprovalCardOutcome::Cancelled,
            ),
        ] {
            for from in [pending(), ApprovalCardState::Submitted { approved: true }] {
                let next = reduce_card(&from, CardAction::Resolved(wire.to_string()));
                assert_eq!(
                    next,
                    ApprovalCardState::Resolved {
                        outcome: outcome.clone()
                    },
                    "{wire} from {from:?}"
                );
            }
            let html = render_card(
                "terminal",
                "r",
                ApprovalCardState::Resolved {
                    outcome: outcome.clone(),
                },
            );
            assert!(html.contains(label), "{label}: {html}");
            assert!(html.contains(tone), "{tone}: {html}");
            assert!(html.contains("role=\"status\""), "{html}");
            assert!(
                !html.contains("<button"),
                "no buttons once resolved: {html}"
            );
        }
        let done = ApprovalCardState::Resolved {
            outcome: ApprovalCardOutcome::Denied,
        };
        assert_eq!(
            reduce_card(&done, CardAction::Click { approved: true }),
            done,
            "a resolved card ignores a click"
        );
        assert_eq!(
            reduce_card(&pending(), CardAction::Resolved("nonsense".into())),
            pending(),
            "an unknown outcome word changes nothing"
        );
    }

    #[test]
    fn approval_renders_once_as_card() {
        let mut bubbles: Vec<ChatBubble> = Vec::new();
        let mut next_id = 1u64;
        let mut cards = ApprovalCards::new();
        for ev in [
            requested("A", "terminal"),
            requested("A", "terminal"),
            resolved("B", "approved"),
            resolved("A", "denied"),
        ] {
            apply_approval_event(&mut bubbles, &mut next_id, &mut cards, &ev);
        }
        let approvals: Vec<_> = bubbles
            .iter()
            .filter(|b| b.kind == ChatBubbleKind::Approval)
            .collect();
        assert_eq!(approvals.len(), 1, "{bubbles:?}");
        assert_eq!(approvals[0].approval_id.as_deref(), Some("A"));
        assert_eq!(
            bubbles
                .iter()
                .filter(|b| b.kind == ChatBubbleKind::Critical)
                .count(),
            0,
            "no critical row for an approval (R4)"
        );
        assert_eq!(
            cards["A"].state,
            ApprovalCardState::Resolved {
                outcome: ApprovalCardOutcome::Denied
            }
        );
        assert_eq!(cards["A"].tool, "terminal");
        assert_eq!(cards["A"].reason, "curl to example.com");
        assert!(!cards.contains_key("B"), "an unknown id adds no entry");
    }

    #[test]
    fn approval_card_css_contract() {
        const SCREENS_CSS: &str = include_str!("../../../../../assets/screens.css");
        let css = css_code(SCREENS_CSS);
        let mut approval_rules = Vec::new();
        for chunk in css.split('}') {
            if let Some((sel, body)) = chunk.rsplit_once('{') {
                if sel.contains("chat-approval") {
                    approval_rules.push((sel.trim().to_string(), body.trim().to_string()));
                }
            }
        }
        assert!(!approval_rules.is_empty(), "no .chat-approval rules");
        let all_bodies: String = approval_rules
            .iter()
            .map(|(_, b)| b.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        let rule = |needle: &str| {
            approval_rules
                .iter()
                .find(|(sel, _)| sel.ends_with(needle))
                .unwrap_or_else(|| panic!("no rule ending `{needle}`: {approval_rules:?}"))
                .1
                .clone()
        };
        let card = rule(".chat-approval-card");
        assert!(card.contains("padding: 16px"), "{card}");
        assert!(card.contains("gap: 8px"), "{card}");
        assert!(
            approval_rules
                .iter()
                .any(|(sel, body)| sel.contains("button") && body.contains("height: 32px")),
            "a card button rule needs height: 32px: {approval_rules:?}"
        );
        let media = css
            .find("@media (max-width: 479px)")
            .expect("a max-width: 479px block");
        let block_end = css[media..]
            .find("\n}")
            .map(|i| media + i)
            .unwrap_or(css.len());
        assert!(
            css[media..block_end].contains("flex: 1 1 128px"),
            "the narrow block must stretch the buttons"
        );
        assert!(all_bodies.contains("opacity: 0.45"), "disabled buttons dim");
        assert!(
            rule(".chat-approval-reason").contains("overflow-wrap: anywhere"),
            "the reason wraps"
        );
        for (sel, var) in [
            (".chat-approval-card.is-amber", "var(--amber)"),
            (".chat-approval-card.is-green", "var(--green)"),
            (".chat-approval-card.is-dim", "var(--text-dim)"),
        ] {
            let body = rule(sel);
            assert!(body.contains(var), "{sel} stripe needs {var}: {body}");
        }
        for (sel, body) in &approval_rules {
            assert!(!body.contains('#'), "`{sel}` has a hex literal: {body}");
            for banned in ["--warn", "--danger", "--accent-primary"] {
                assert!(!body.contains(banned), "`{sel}` uses {banned}: {body}");
            }
        }
    }

    fn model(state: ApprovalCardState) -> ApprovalCardModel {
        ApprovalCardModel {
            session_id: "s".into(),
            tool: "terminal".into(),
            reason: "r".into(),
            state,
        }
    }

    const ERROR_LINE: &str =
        "Could not reach the server. The request will time out and be denied. You can try again.";

    /// R10: `Ok(false)` is a normal outcome, not an error: the card says nothing was changed.
    #[test]
    fn approval_card_ok_false_shows_no_longer_pending() {
        let expired = ApprovalCardState::Resolved {
            outcome: ApprovalCardOutcome::Expired,
        };
        for from in [
            ApprovalCardState::Submitted { approved: true },
            ApprovalCardState::Pending { error: true },
        ] {
            assert_eq!(
                reduce_card(
                    &from,
                    CardAction::ServerReply {
                        approved: true,
                        resolved: false
                    }
                ),
                expired,
                "from {from:?}"
            );
        }
        let html = render_card("terminal", "r", expired.clone());
        for needle in [
            "NO LONGER PENDING",
            "This approval was already answered, timed out, or its turn ended. Nothing was changed.",
            "is-dim",
        ] {
            assert!(html.contains(needle), "missing `{needle}` in {html}");
        }
        assert!(!html.contains("<button"), "no buttons once expired: {html}");
        assert_eq!(
            reduce_card(&expired, CardAction::Resolved("denied".into())),
            ApprovalCardState::Resolved {
                outcome: ApprovalCardOutcome::Denied
            },
            "the server's real resolution replaces Expired"
        );
        let approved = ApprovalCardState::Resolved {
            outcome: ApprovalCardOutcome::Approved,
        };
        assert_eq!(
            reduce_card(&approved, CardAction::Resolved("denied".into())),
            approved,
            "any other resolved state stays terminal"
        );
    }

    /// R10: an `Err` brings the buttons back so the operator can retry.
    #[test]
    fn approval_card_error_reenables_buttons() {
        let back = reduce_card(
            &ApprovalCardState::Submitted { approved: false },
            CardAction::ServerErr,
        );
        assert_eq!(back, ApprovalCardState::Pending { error: true });
        let html = render_card("terminal", "r", back.clone());
        assert!(html.contains(ERROR_LINE), "{html}");
        assert_eq!(html.matches("<button").count(), 2, "{html}");
        assert!(
            !html.contains("disabled"),
            "buttons are enabled again: {html}"
        );
        assert!(html.contains("is-amber"), "{html}");
        assert_eq!(
            reduce_card(&back, CardAction::Click { approved: true }),
            ApprovalCardState::Submitted { approved: true },
            "a retry goes to Submitted again"
        );
        assert!(
            !render_card("terminal", "r", pending()).contains(ERROR_LINE),
            "a fresh card shows no error line"
        );
    }

    /// UI-SPEC Interaction / E3 drop while Submitted: no card is ever left disabled after a
    /// socket drop, and every exit from the error state is reachable.
    #[test]
    fn approval_card_ws_drop_shows_error_keeps_buttons() {
        let err = ApprovalCardState::Pending { error: true };
        for from in [pending(), ApprovalCardState::Submitted { approved: true }] {
            assert_eq!(
                reduce_card(&from, CardAction::Disconnected),
                err,
                "{from:?}"
            );
        }
        let done = ApprovalCardState::Resolved {
            outcome: ApprovalCardOutcome::Approved,
        };
        assert_eq!(reduce_card(&done, CardAction::Disconnected), done);

        let html = render_card("terminal", "r", err.clone());
        assert!(html.contains(ERROR_LINE), "{html}");
        assert_eq!(html.matches("<button").count(), 2, "{html}");
        assert!(!html.contains("disabled"), "{html}");

        // A click, then the server says the approval is gone.
        let clicked = reduce_card(&err, CardAction::Click { approved: true });
        assert_eq!(
            reduce_card(
                &clicked,
                CardAction::ServerReply {
                    approved: true,
                    resolved: false
                }
            ),
            ApprovalCardState::Resolved {
                outcome: ApprovalCardOutcome::Expired
            }
        );
        // A click, then the server accepts it.
        assert_eq!(
            reduce_card(
                &clicked,
                CardAction::ServerReply {
                    approved: true,
                    resolved: true
                }
            ),
            done
        );
        // The original in-flight reply lands after the drop, with no new click.
        assert_eq!(
            reduce_card(
                &err,
                CardAction::ServerReply {
                    approved: true,
                    resolved: true
                }
            ),
            done
        );

        let mut cards = ApprovalCards::new();
        cards.insert("p".into(), model(pending()));
        cards.insert(
            "s".into(),
            model(ApprovalCardState::Submitted { approved: false }),
        );
        cards.insert("r".into(), model(done.clone()));
        assert!(any_approval_pending(&cards));
        assert!(apply_disconnect(&mut cards));
        assert!(
            !any_approval_pending(&cards),
            "after a drop no card lights the badge"
        );
        assert_eq!(cards["p"].state, err);
        assert_eq!(cards["s"].state, err);
        assert_eq!(cards["r"].state, done, "a resolved card is untouched");
        assert!(
            !apply_disconnect(&mut cards),
            "a second drop changes nothing"
        );
    }

    #[test]
    fn session_scoped_disconnect_leaves_other_sessions_alone() {
        // RR-03: the OpenUI socket dropping must not flag Chat's cards.
        let mut cards = ApprovalCards::new();
        let mut chat = model(ApprovalCardState::Pending { error: false });
        chat.session_id = "chat".into();
        let mut ui = model(ApprovalCardState::Pending { error: false });
        ui.session_id = "openui".into();
        cards.insert("c".into(), chat);
        cards.insert("u".into(), ui);
        assert!(apply_disconnect_for_session(&mut cards, "openui"));
        assert_eq!(
            cards["u"].state,
            ApprovalCardState::Pending { error: true }
        );
        assert_eq!(
            cards["c"].state,
            ApprovalCardState::Pending { error: false },
            "Chat's card is untouched"
        );
        assert!(!apply_disconnect_for_session(&mut cards, "openui"));
        assert!(!apply_disconnect_for_session(&mut cards, ""), "empty id matches no card");
    }

    #[test]
    fn approval_card_source_contract() {
        const SRC: &str = include_str!("approval_card.rs");
        let code = rust_code(&SRC[..SRC.find(concat!("#[cfg(", "test)]")).unwrap()]);
        assert!(
            code.contains(concat!("resolve_web_approval", "(")),
            "the buttons must call the authenticated server fn"
        );
        for banned in [
            concat!(".fo", "cus("),
            concat!("set_", "focus"),
            concat!("auto", "focus"),
            concat!("on", "mounted"),
            concat!("Html", "AudioElement"),
            concat!("audio", " {"),
            concat!("Agent", "NoticeCtx"),
        ] {
            assert!(
                !code.contains(banned),
                "approval_card.rs contains `{banned}`"
            );
        }
    }
}
