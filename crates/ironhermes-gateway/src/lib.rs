pub mod adapter;
pub mod backoff;
pub mod handler;
pub mod multimodal;
pub mod pid;
pub mod rate_limiter;
pub mod session;
pub mod stream_consumer;
pub mod telegram;
pub mod runner;
pub mod user_queue;

pub use adapter::{PlatformAdapter, MessageHandler};
pub use backoff::BackoffState;
pub use handler::GatewayMessageHandler;
pub use session::GatewaySession;
pub use stream_consumer::StreamConsumer;
pub use runner::{GatewayRunner, dispatch_delivery};
pub use user_queue::UserQueueManager;
pub use telegram::{TelegramAdapter, TgMessage, TgUser, TgChat, TgUpdate, TgBotCommand, TgFile, TgPhotoSize, TgDocument, TgSendApi};
pub use pid::{
    acquire_pid_lock, is_pid_alive, read_gateway_pid, write_gateway_pid,
    GatewayPidRecord, PidLiveness, PidLockGuard,
};
