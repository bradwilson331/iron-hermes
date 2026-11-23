//! Phase 54.8 D-07 — the runner-owned permit path.
//!
//! Before this module the per-chat worker and the slash-command fast path in
//! `runner.rs` each acquired one of the `max_concurrent_runs` permits inline and
//! held it for as long as `handle_with_multimodal` took. A hung adapter call
//! anywhere in that function therefore pinned a permit forever, and eight of
//! them froze the whole bot — including `/stop` and `/approve`, which took a
//! permit from the same semaphore.
//!
//! [`run_turn_slot`] is now the single owner of the permit. It runs the whole
//! call (the caller's `prepare` step plus the handler) under an absolute
//! ceiling (`TurnBudgets::handler_ceiling`); on expiry it cancels that call's
//! token, drops the future and releases the permit. [`run_slash_command`] runs
//! the control commands (`/stop`, `/approve`, `/deny`) in a lane that takes no
//! permit at all, so they stay usable while every run slot is busy.
//!
//! Both runner paths call these functions, and the integration tests in
//! `tests/turn_deadline.rs` drive the same functions with a real `Semaphore`.

use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use ironhermes_core::MessageEvent;
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;
use tracing::error;

use crate::adapter::PlatformAdapter;
use crate::handler::GatewayMessageHandler;
use crate::multimodal::ProcessedAttachments;

/// Budget for a control command (`/stop`, `/approve`, `/deny`) running in the
/// permit-free lane. Their handlers are short; this only bounds a hung adapter.
pub const CONTROL_COMMAND_BUDGET: Duration = Duration::from_secs(30);

/// How one gateway call ended.
#[derive(Debug)]
pub enum SlotOutcome {
    /// The handler returned (successfully or with its own error).
    Completed(anyhow::Result<()>),
    /// The absolute ceiling (or the control-command budget) elapsed; the call
    /// was cancelled and dropped.
    CeilingHit,
    /// The semaphore was closed (shutdown in progress); nothing ran.
    SemaphoreClosed,
}

/// True when the first word of `content`, with any `@bot` suffix removed, is
/// exactly `/stop`, `/approve` or `/deny`.
pub fn is_control_command(content: &str) -> bool {
    let first = content.split_whitespace().next().unwrap_or("");
    let base = first.split('@').next().unwrap_or("");
    matches!(base, "/stop" | "/approve" | "/deny")
}

fn no_attachments() -> ProcessedAttachments {
    ProcessedAttachments {
        text_prefix: None,
        image_data_uri: None,
        image_cache_path: None,
    }
}

/// Acquire one run permit and run one gateway call under the absolute ceiling.
///
/// `prepare` runs INSIDE the ceiling and after the permit is held (the worker
/// uses it for the eyes reaction and the multimodal sidecar), so a hang in
/// that step cannot pin the permit either. The permit is released when this
/// function returns, on every path.
pub async fn run_turn_slot<F, Fut>(
    semaphore: &Arc<Semaphore>,
    handler: &Arc<GatewayMessageHandler>,
    event: &MessageEvent,
    adapter: Arc<dyn PlatformAdapter>,
    parent_cancel: &CancellationToken,
    prepare: F,
) -> SlotOutcome
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = ProcessedAttachments>,
{
    let permit = match semaphore.acquire().await {
        Ok(p) => p,
        Err(_) => return SlotOutcome::SemaphoreClosed,
    };
    let call_cancel = parent_cancel.child_token();

    let work = async {
        let processed = prepare().await;
        handler
            .handle_with_multimodal(event, adapter, call_cancel.clone(), processed)
            .await
    };

    let outcome = match handler.turn_budgets().handler_ceiling() {
        Some(ceiling) => match tokio::time::timeout(ceiling, work).await {
            Ok(result) => SlotOutcome::Completed(result),
            Err(_) => {
                // Stops the typing task and anything else on this call's token.
                call_cancel.cancel();
                error!(
                    chat_id = %event.chat_id,
                    ceiling_secs = ceiling.as_secs(),
                    "gateway call exceeded its ceiling; dropped (D-07)"
                );
                SlotOutcome::CeilingHit
            }
        },
        None => SlotOutcome::Completed(work.await),
    };
    drop(permit);
    outcome
}

/// Run a slash command from the runner's fast path.
///
/// Control commands take NO permit and run under [`CONTROL_COMMAND_BUDGET`];
/// every other command goes through [`run_turn_slot`] with empty attachments.
pub async fn run_slash_command(
    semaphore: &Arc<Semaphore>,
    handler: &Arc<GatewayMessageHandler>,
    event: &MessageEvent,
    adapter: Arc<dyn PlatformAdapter>,
    parent_cancel: &CancellationToken,
) -> SlotOutcome {
    if is_control_command(&event.content) {
        let cancel = parent_cancel.child_token();
        let work = handler.handle_with_multimodal(event, adapter, cancel.clone(), no_attachments());
        return match tokio::time::timeout(CONTROL_COMMAND_BUDGET, work).await {
            Ok(result) => SlotOutcome::Completed(result),
            Err(_) => {
                cancel.cancel();
                error!(
                    chat_id = %event.chat_id,
                    budget_secs = CONTROL_COMMAND_BUDGET.as_secs(),
                    "control command exceeded its budget; dropped (D-07)"
                );
                SlotOutcome::CeilingHit
            }
        };
    }
    run_turn_slot(
        semaphore,
        handler,
        event,
        adapter,
        parent_cancel,
        || async { no_attachments() },
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_control_command_cases() {
        for yes in ["/stop", "/approve", "/deny", "/stop@demo_bot", "/approve 3"] {
            assert!(is_control_command(yes), "{yes} is a control command");
        }
        for no in ["/help", "/mcp connect x", "stop", "", "  ", "/stopper"] {
            assert!(!is_control_command(no), "{no:?} is not a control command");
        }
    }
}
