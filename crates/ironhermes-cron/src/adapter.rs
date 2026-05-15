use async_trait::async_trait;

/// Minimal send trait used by the cron-runner crate to dispatch
/// delivery payloads. Implemented by `TelegramAdapter`
/// (ironhermes-gateway) for production and `FakeTgClient` for tests.
///
/// Lives in `ironhermes-cron` (not `ironhermes-gateway`) so that
/// `ironhermes-cron-runner` can depend on the trait without forming
/// a dependency cycle with the gateway.
#[async_trait]
pub trait TgSendApi: Send + Sync {
    async fn send_message(
        &self,
        chat_id: &str,
        content: &str,
        thread_id: Option<&str>,
    ) -> anyhow::Result<()>;
}
