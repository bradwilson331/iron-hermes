pub mod adapter;
pub mod backoff;
pub mod handler;
pub mod multimodal;
pub mod pid;
pub mod rate_limiter;
pub mod runner;
pub mod session;
pub mod stream_consumer;
pub mod telegram;
pub mod user_queue;

pub use adapter::{MessageHandler, PlatformAdapter};
pub use backoff::BackoffState;
pub use handler::GatewayMessageHandler;
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
