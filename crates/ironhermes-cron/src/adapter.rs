use std::path::Path;
use async_trait::async_trait;

/// Minimal send trait used by the cron-runner crate to dispatch
/// delivery payloads. Implemented by `TelegramAdapter`
/// (ironhermes-gateway) for production and `FakeTgClient` for tests.
///
/// Lives in `ironhermes-cron` (not `ironhermes-gateway`) so that
/// `ironhermes-cron-runner` can depend on the trait without forming
/// a dependency cycle with the gateway.
///
/// The four media methods (`send_voice`, `send_image_file`,
/// `send_video`, `send_document`) were added in Plan 32.1-07 Task 3
/// to support MEDIA tag routing in `dispatch_all_targets`.
#[async_trait]
pub trait TgSendApi: Send + Sync {
    async fn send_message(
        &self,
        chat_id: &str,
        content: &str,
        thread_id: Option<&str>,
    ) -> anyhow::Result<()>;

    /// Send a voice message (ogg/opus/mp3/wav/m4a).
    async fn send_voice(
        &self,
        chat_id: &str,
        path: &Path,
        thread_id: Option<&str>,
    ) -> anyhow::Result<()>;

    /// Send a photo/image (png/jpg/jpeg/gif/webp).
    async fn send_image_file(
        &self,
        chat_id: &str,
        path: &Path,
        thread_id: Option<&str>,
    ) -> anyhow::Result<()>;

    /// Send a video (mp4/mov/webm/mkv).
    async fn send_video(
        &self,
        chat_id: &str,
        path: &Path,
        thread_id: Option<&str>,
    ) -> anyhow::Result<()>;

    /// Send a document (any file not covered by the above types).
    async fn send_document(
        &self,
        chat_id: &str,
        path: &Path,
        thread_id: Option<&str>,
    ) -> anyhow::Result<()>;
}
