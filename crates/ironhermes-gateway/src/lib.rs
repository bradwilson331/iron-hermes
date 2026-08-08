pub mod adapter;
/// Phase 45: async approval engine (ApprovalCoordinator + GatewayApprovalGate).
pub mod approval;
pub mod backoff;
/// Phase 47.6 Plan 03 (P0-1): the pure, unit-testable boot gate
/// (`resolve_enabled_platforms`) that decides which platforms are usable
/// before `GatewayRunner::start` spawns any adapter.
pub mod boot_gate;
#[cfg(feature = "buzz")]
pub mod buzz; // Phase 47.6 Plan 01 — P0-3
#[cfg(feature = "buzz")]
pub mod buzz_identity; // Phase 47.6 Plan 01 — D-06
#[cfg(feature = "buzz")]
pub mod buzz_send; // Phase 47.6 Plan 07 — DeliverySend impl for BuzzAdapter
pub mod discord; // Phase 34 — D-10
pub mod handler;
pub mod markdown_v2; // Phase 36.17.2.2 — D-04 Telegram MarkdownV2 escape
pub mod media_tag; // Phase 36.17.2.2 — D-05/D-06/D-08/D-09 streaming MediaTagExtractor
pub mod multimodal;
// Phase 36.3.7.5 BUG-36.3.7.5-04: pure-function notifier-spawn gate. `pub` is for
// receiver-end integration tests only (see `tests/notifier_spawn_gating.rs`).
pub mod notifier_gating;
pub mod pid;
pub mod rate_limiter;
pub mod runner;
pub mod session;
pub mod session_queue; // Phase 36.17.1 — per-session FIFO queue (Python parity: gateway/run.py §2304-2415)
pub mod slack; // Phase 34 — D-11
pub mod stream_consumer;
pub mod telegram;
pub mod user_queue;

/// RC-1 / REQ-37.2-01 / REQ-37.2-02: turn-end fallback delivery for live (gateway) turns.
///
/// Called when `final_body` (the text streamed to the placeholder) is empty after `flush(true)`.
/// Implements the decision tree:
///
/// - `final_body` empty + `final_response` Some(non-empty) → edit placeholder with
///   `escape_outside_code_blocks(final_response)` (REQ-37.2-01).
/// - `final_body` empty + `final_response` None or empty → `delete_message(placeholder_id)`
///   (REQ-37.2-02 — no bare `█` left behind).
///
/// This is extracted as a testable helper so `turn_end_delivery.rs` integration tests can
/// exercise the RC-1 fallback decision tree without needing a live `run_agent` invocation.
/// The same logic is called from `handler.rs::run_agent` after the D-10 media block
/// (guarded by `placeholder_handled_by_d10` to prevent Pitfall 5 double-edit).
///
/// # Security
/// `final_response` is run through `escape_outside_code_blocks` before the edit call
/// (T-37.2-02-01 / ASVS V5 input validation) — identical to the normal stream path.
pub async fn deliver_turn_end_fallback(
    adapter: &dyn adapter::PlatformAdapter,
    chat_id: &str,
    placeholder_id: &str,
    final_body: &str,
    final_response: Option<&str>,
) -> anyhow::Result<()> {
    if !final_body.trim().is_empty() {
        // Non-empty body: normal stream path already flushed the placeholder.
        // Caller should not invoke this function in that case, but be safe.
        return Ok(());
    }

    match final_response {
        Some(fr) if !fr.trim().is_empty() => {
            // REQ-37.2-01: edit placeholder with escaped final_response
            let escaped = markdown_v2::escape_outside_code_blocks(fr);
            let _ = adapter
                .edit_message_markdown_v2(chat_id, placeholder_id, &escaped)
                .await;
        }
        _ => {
            // REQ-37.2-02: truly empty turn — delete the placeholder
            let _ = adapter.delete_message(chat_id, placeholder_id).await;
        }
    }

    Ok(())
}

pub use adapter::{MessageHandler, PlatformAdapter};
pub use backoff::BackoffState;
#[cfg(feature = "buzz")]
pub use buzz::{BuzzAdapter, run_buzz_adapter};
pub use discord::{DiscordAdapter, run_discord_adapter};
pub use handler::GatewayMessageHandler;
pub use pid::{
    GatewayPidRecord, PidLiveness, PidLockGuard, acquire_pid_lock, is_pid_alive, read_gateway_pid,
    write_gateway_pid,
};
pub use runner::GatewayRunner;
pub use slack::{SlackAdapter, run_slack_adapter};
// Note: dispatch_delivery (Plan 22.4.2.1) was removed in Plan 32.1-07.
// Delivery dispatch is now handled by ironhermes_cron_runner::dispatch_all_targets.
pub use ironhermes_cron::{DeliveryRegistry, DeliverySend, TgSendApi};
pub use session::GatewaySession;
pub use stream_consumer::StreamConsumer;
pub use telegram::{
    TelegramAdapter, TgBotCommand, TgChat, TgDocument, TgFile, TgMessage, TgPhotoSize, TgUpdate,
    TgUser,
};
pub use user_queue::UserQueueManager;
