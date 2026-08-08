//! Phase 01-04 (DLV-03 web) — WebImageDispatcher: web-runtime image dispatcher.
//!
//! Reads a generated image file from the images cache directory, extracts the
//! UUID from the file stem, infers the MIME from the extension, and emits a
//! `ChatStreamEvent::ImageOut` via the per-turn unbounded channel so the ws
//! send loop can forward it to the browser as a `Message::Binary` frame.
//!
//! This mirrors `WebAudioDispatcher` (same directory): the audio path delivers
//! synthesized mp3 bytes as binary `AudioOut` frames; the image path delivers
//! generated image bytes as binary `ImageOut` frames. The orphan-rule note
//! that forced `WebAudioDispatcher` to live here (not in `ironhermes-tools`)
//! does not apply — there is no `ImageDispatcher` trait; the ws turn loop
//! calls `send_image_file` directly with an extracted `MediaRef` path. The
//! dispatcher is kept here for symmetry with the audio path.
//!
//! Do NOT re-persist image bytes here — `image_gen` / `FalClient` already wrote
//! the file to `~/.ironhermes/cache/images/`. This dispatcher reads only.

#![cfg(feature = "server")]

use std::path::{Path, PathBuf};
use tokio::sync::mpsc::UnboundedSender;

use crate::protocol::ChatStreamEvent;

/// Photo size cap (T-04-03 DoS mitigation; DLV-04 parity). Mirrors the
/// gateway's 20 MB Telegram photo cap (`SIZE_CAP_20_MB` in
/// `ironhermes-gateway::telegram`) and `PHOTO_SIZE_CAP` in
/// `ironhermes-tools::fal`. An image at or under this size is binary-framed to
/// the browser; anything larger is skipped (not silently framed over WS).
const PHOTO_SIZE_CAP: u64 = 20 * 1024 * 1024;

/// Web-runtime image dispatcher.
///
/// Constructed per-turn in `ws.rs` with a clone of the turn's `tx` sender and
/// the images cache directory path. On `send_image_file`, reads the image
/// bytes from disk, extracts the UUID from the file stem, infers the MIME from
/// the extension, and emits a `ChatStreamEvent::ImageOut` into the channel.
///
/// The ws send loop's `Message::Binary` arm picks up ImageOut events and
/// forwards them to the browser client as binary WebSocket frames.
pub struct WebImageDispatcher {
    tx: UnboundedSender<ChatStreamEvent>,
    #[allow(dead_code)]
    // reserved for a future served-path route; binary-frame transport reads only
    images_cache_dir: PathBuf,
}

impl WebImageDispatcher {
    /// Construct a new dispatcher for a single web turn.
    ///
    /// `tx` — the per-turn unbounded sender for `ChatStreamEvent`; a clone of
    ///        the same `tx` used by the stream/tool callbacks in `ws.rs`.
    /// `images_cache_dir` — the directory where `image_gen` writes generated
    ///        images (`get_hermes_home()/cache/images`).
    pub fn new(tx: UnboundedSender<ChatStreamEvent>, images_cache_dir: PathBuf) -> Self {
        Self {
            tx,
            images_cache_dir,
        }
    }

    /// Read an image from `path`, emit a `ChatStreamEvent::ImageOut` event.
    ///
    /// Extracts the UUID from the file stem (the filename without extension)
    /// and the MIME from the extension. Does NOT re-persist bytes — reads only.
    /// Images larger than [`PHOTO_SIZE_CAP`] are skipped (T-04-03) so an
    /// oversized image is never binary-framed over the WS.
    pub async fn send_image_file(&self, path: &Path) -> anyhow::Result<()> {
        // Size-gate before reading the whole file into memory (T-04-03).
        let metadata = tokio::fs::metadata(path)
            .await
            .map_err(|e| anyhow::anyhow!("stat image file {}: {e}", path.display()))?;
        if metadata.len() > PHOTO_SIZE_CAP {
            tracing::warn!(
                path = %path.display(),
                size = metadata.len(),
                cap = PHOTO_SIZE_CAP,
                "WebImageDispatcher: image exceeds photo cap; skipping ImageOut frame"
            );
            return Ok(());
        }

        // Read image bytes from disk. image_gen/FalClient already persisted;
        // WebImageDispatcher is a read-only consumer.
        let bytes = tokio::fs::read(path)
            .await
            .map_err(|e| anyhow::anyhow!("read image file {}: {e}", path.display()))?;

        // Extract UUID from file stem (e.g. "abc123" from "abc123.png").
        let uuid = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("unknown")
            .to_string();

        let mime = mime_for_extension(path);

        let event = ChatStreamEvent::ImageOut { mime, uuid, bytes };

        self.tx
            .send(event)
            .map_err(|e| anyhow::anyhow!("ws send failed: {e}"))?;

        Ok(())
    }
}

/// Infer an image MIME type from a path's extension. Mirrors the MediaKind
/// photo extensions (png/jpg/jpeg/webp/gif). Falls back to `image/png`.
fn mime_for_extension(path: &Path) -> String {
    let ext = path
        .extension()
        .and_then(|s| s.to_str())
        .map(|s| s.to_ascii_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "gif" => "image/gif",
        "png" => "image/png",
        _ => "image/png",
    }
    .to_string()
}

#[cfg(test)]
mod tests {
    //! Behavioral coverage for WebImageDispatcher.
    //!
    //! Lives here (not in `tests/`) because `iron_hermes_ui` is a binary crate
    //! with no `[lib]` target — integration tests cannot
    //! `use iron_hermes_ui::server::...`. As an in-source unit test it names
    //! the crate-private types directly (mirrors web_audio_dispatcher tests).
    use super::{WebImageDispatcher, PHOTO_SIZE_CAP};
    use crate::protocol::ChatStreamEvent;
    use tempfile::TempDir;
    use tokio::sync::mpsc;

    /// Phase 01-04 (DLV-03): dispatcher reads image bytes, emits ImageOut event
    /// with mime inferred from the extension, uuid=file-stem, bytes=contents.
    #[tokio::test]
    async fn dispatcher_sends_image_out_on_send_image_file() {
        let (tx, mut rx) = mpsc::unbounded_channel::<ChatStreamEvent>();

        let temp_dir = TempDir::new().expect("create temp dir");
        let uuid = "00000000-0000-0000-0000-000000000000";
        let png_path = temp_dir.path().join(format!("{uuid}.png"));

        // 8-byte PNG signature prefix is enough to assert byte passthrough.
        let expected_bytes: Vec<u8> = vec![0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
        std::fs::write(&png_path, &expected_bytes).expect("write temp png");

        let dispatcher = WebImageDispatcher::new(tx, temp_dir.path().to_path_buf());

        let result = dispatcher.send_image_file(&png_path).await;
        assert!(result.is_ok(), "send_image_file must return Ok(())");

        let event = rx.recv().await.expect("must receive ImageOut event");
        match event {
            ChatStreamEvent::ImageOut {
                mime,
                uuid: got_uuid,
                bytes,
            } => {
                assert_eq!(mime, "image/png", "DLV-03: PNG extension → image/png");
                assert_eq!(got_uuid, uuid, "DLV-03: ImageOut uuid must match file stem");
                assert_eq!(
                    bytes, expected_bytes,
                    "DLV-03: ImageOut bytes must match file contents"
                );
            }
            other => panic!("Expected ChatStreamEvent::ImageOut, got {other:?}"),
        }
    }

    /// T-04-03: an image larger than the photo cap is skipped (no frame sent),
    /// never binary-framed over the WS.
    #[tokio::test]
    async fn oversized_image_is_skipped() {
        let (tx, mut rx) = mpsc::unbounded_channel::<ChatStreamEvent>();

        let temp_dir = TempDir::new().expect("create temp dir");
        let png_path = temp_dir.path().join("big.png");

        // Write one byte over the cap (sparse-ish: just exceed the threshold).
        let oversized = vec![0u8; (PHOTO_SIZE_CAP + 1) as usize];
        std::fs::write(&png_path, &oversized).expect("write oversized png");

        let dispatcher = WebImageDispatcher::new(tx, temp_dir.path().to_path_buf());

        let result = dispatcher.send_image_file(&png_path).await;
        assert!(
            result.is_ok(),
            "oversized image returns Ok (skipped, not error)"
        );
        assert!(
            rx.try_recv().is_err(),
            "T-04-03: no ImageOut frame for an oversized image"
        );
    }
}
