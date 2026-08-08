//! Phase 47.6 Plan 07: `DeliverySend` impl for `BuzzAdapter`.
//!
//! `DeliverySend` (`ironhermes-cron::adapter`) is the platform-keyed
//! text-delivery trait the cron registry and (via `TelegramAdapter`'s own
//! impl in `telegram.rs`) the notifier's uniform text path both resolve
//! through. `BuzzAdapter` is defined in `ironhermes-gateway::buzz`, so this
//! impl must live where the TYPE is local (the orphan rule permits `impl
//! Trait for Type` only when the trait or the type is local to the crate;
//! `DeliverySend` lives in `ironhermes-cron`, so `BuzzAdapter` being local
//! here is what makes this compile) — `telegram.rs` carries the identical
//! precedent for `TgSendApi`/`TelegramAdapter`.
//!
//! Kept in its own module (rather than folded into `buzz.rs`) so the
//! transport module stays free of cron-registry concerns, and so this
//! plan's diff never touches `buzz.rs` at all.
//!
//! D-15: no `MediaSender` implementation for Buzz here or anywhere this
//! phase — media arrives as a path-naming text notice built by
//! `ironhermes-cron-runner::delivery::buzz_media_notice` and sent as a
//! second `send_text` call, not through a media-specific trait method.

use crate::buzz::BuzzAdapter;
use async_trait::async_trait;
use ironhermes_core::MessageResponse;
use ironhermes_cron::DeliverySend;

#[async_trait]
impl DeliverySend for BuzzAdapter {
    async fn send_text(
        &self,
        chat_id: &str,
        content: &str,
        thread_id: Option<&str>,
    ) -> anyhow::Result<()> {
        // Delegate to PlatformAdapter::send_message; discard MessageResponse
        // (mirrors TelegramAdapter's own DeliverySend impl in telegram.rs).
        <BuzzAdapter as crate::adapter::PlatformAdapter>::send_message(
            self, chat_id, content, thread_id,
        )
        .await
        .map(|_: MessageResponse| ())
    }
}
