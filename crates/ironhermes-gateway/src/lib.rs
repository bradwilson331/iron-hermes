pub mod adapter;
pub mod backoff;
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

pub use adapter::{MessageHandler, PlatformAdapter};
pub use discord::{DiscordAdapter, run_discord_adapter};
pub use slack::{SlackAdapter, run_slack_adapter};
pub use backoff::BackoffState;
pub use handler::GatewayMessageHandler;
pub use ironhermes_core::commands::running_agent::RunningAgentGuard;
pub use pid::{
    GatewayPidRecord, PidLiveness, PidLockGuard, acquire_pid_lock, is_pid_alive, read_gateway_pid,
    write_gateway_pid,
};
pub use runner::GatewayRunner;
// Note: dispatch_delivery (Plan 22.4.2.1) was removed in Plan 32.1-07.
// Delivery dispatch is now handled by ironhermes_cron_runner::dispatch_all_targets.
pub use session::GatewaySession;
pub use stream_consumer::StreamConsumer;
pub use ironhermes_cron::TgSendApi;
pub use telegram::{
    TelegramAdapter, TgBotCommand, TgChat, TgDocument, TgFile, TgMessage, TgPhotoSize, TgUpdate,
    TgUser,
};
pub use user_queue::UserQueueManager;
