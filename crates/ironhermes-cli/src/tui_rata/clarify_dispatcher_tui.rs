//! Channel-based clarify dispatcher for the `tui_rata` REPL (Phase 41.1 Plan
//! 10, G-41.1-1).
//!
//! # Why this exists (G-41.1-1 root cause)
//!
//! `ClarifyTool::execute_clarify`'s no-dispatcher fallback
//! (`ironhermes-tools/src/clarify_tool.rs:165-171`) performs raw `println!()`
//! writes directly to process stdout. `tui_rata` previously wired
//! `clarify_dispatcher: None` unconditionally (`event_loop.rs`), so every
//! model-invoked `clarify` call took that branch — from a task running
//! concurrently and unsynchronized with the main render loop's own
//! `terminal.draw()` over the same raw-mode + alternate-screen stdout,
//! desyncing ratatui's internal buffer model and corrupting the transcript
//! (see `.planning/debug/41.1-tui-interactive-render-corruption.md`).
//!
//! This dispatcher mirrors
//! [`TuiApprovalGate`](crate::tui_rata::approval_gate_tui::TuiApprovalGate)
//! exactly: `send_question` (called from WITHIN the spawned agent-turn task)
//! sends a [`ClarifyRequest`] down an `UnboundedSender` that the main loop
//! drains via a `tokio::select!` arm and surfaces as a ratatui overlay
//! (`App::surface_clarify_request`). The answer routes back through the
//! SHARED `PendingClarifyRegistry` — NOT a channel on this struct —
//! `App::answer_clarify`/`cancel_clarify` call `registry.take`/
//! `registry.remove` directly, since the registry (not this dispatcher) owns
//! the `oneshot::Sender` that unblocks the suspended `execute_clarify` call.
//!
//! # Fail path
//!
//! `send_question` returns `Err` when the receiver (main loop) is gone — the
//! tool then cleans up its own registry entry and propagates the error (see
//! `clarify_tool.rs::execute_clarify`'s dispatcher-error branch). No stdout
//! write ever happens on this path.

use async_trait::async_trait;
use tokio::sync::mpsc::UnboundedSender;

use ironhermes_tools::ClarifyDispatcher;

/// A clarify question surfaced from a spawned agent-turn task to the TUI
/// decision surface. Unlike
/// [`ApprovalRequest`](crate::tui_rata::approval_gate_tui::ApprovalRequest),
/// this carries NO reply channel — the answer routes back through the shared
/// `PendingClarifyRegistry` keyed by `clarify_id`, not a `oneshot::Sender`
/// stashed on the request itself.
pub struct ClarifyRequest {
    /// The question text to present to the user.
    pub question: String,
    /// The option labels the user chooses from (2-10 per the tool schema).
    pub choices: Vec<String>,
    /// The registry key (`"clarify:<chat_id>"`) — `App::answer_clarify`/
    /// `cancel_clarify` pass this straight to `PendingClarifyRegistry::take`/
    /// `remove` to resolve the exact suspended turn that inserted it.
    pub clarify_id: String,
}

/// Channel-based [`ClarifyDispatcher`] for `tui_rata`. Holds the sender half
/// of the request channel wired to `App.clarify_rx` (drained by
/// `recv_clarify_request` in `run_app_inner`'s `select!`).
pub struct TuiClarifyDispatcher {
    tx: UnboundedSender<ClarifyRequest>,
}

impl TuiClarifyDispatcher {
    /// `tx`: the sender half wired to `App.clarify_rx`.
    pub fn new(tx: UnboundedSender<ClarifyRequest>) -> Self {
        Self { tx }
    }
}

#[async_trait]
impl ClarifyDispatcher for TuiClarifyDispatcher {
    async fn send_question(
        &self,
        _chat_id: &str,
        _thread_id: Option<&str>,
        question: &str,
        choices: &[String],
        clarify_id: &str,
    ) -> anyhow::Result<()> {
        self.tx
            .send(ClarifyRequest {
                question: question.to_string(),
                choices: choices.to_vec(),
                clarify_id: clarify_id.to_string(),
            })
            .map_err(|_| anyhow::anyhow!("clarify: TUI overlay receiver is gone"))
    }
}

#[cfg(all(test, feature = "test-support"))]
mod tests {
    use super::*;

    /// Proves `send_question` delivers question/choices/clarify_id over the
    /// mpsc channel (into the overlay path) — NOT to stdout. This is the
    /// direct fix for G-41.1-1: the same data that used to reach
    /// `clarify_tool.rs`'s raw `println!` fallback now reaches
    /// `App::surface_clarify_request` via this channel instead. Mirrors
    /// `approval_gate_tui.rs`'s `tui_gate_channel_roundtrip_approve`.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn clarify_dispatcher_channel_roundtrip() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<ClarifyRequest>();
        let dispatcher = TuiClarifyDispatcher::new(tx);

        let choices = vec!["Option A".to_string(), "Option B".to_string()];
        let result = dispatcher
            .send_question("chat-1", None, "Pick one", &choices, "clarify:chat-1")
            .await;
        assert!(result.is_ok());

        let req = rx.recv().await.expect("request surfaced on the channel");
        assert_eq!(req.question, "Pick one");
        assert_eq!(req.choices, choices);
        assert_eq!(req.clarify_id, "clarify:chat-1");
    }

    /// A dropped receiver (main loop gone) must surface as `Err` so
    /// `execute_clarify` cleans up its registry entry and propagates — fail
    /// visibly, never silently drop the question (mirrors `TuiApprovalGate`'s
    /// fail-closed discipline on a dropped channel).
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn clarify_dispatcher_errs_when_receiver_dropped() {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<ClarifyRequest>();
        drop(rx);
        let dispatcher = TuiClarifyDispatcher::new(tx);

        let result = dispatcher
            .send_question(
                "chat-1",
                None,
                "Pick one",
                &["A".to_string()],
                "clarify:chat-1",
            )
            .await;
        assert!(
            result.is_err(),
            "a dropped receiver must surface as Err, never silently succeed"
        );
    }
}
