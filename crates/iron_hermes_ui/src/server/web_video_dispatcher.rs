//! Phase 36.3.3 (D-08 web) — WebVideoDispatcher: web-runtime video dispatcher.
//!
//! Reads a generated video file from the videos cache directory, extracts the
//! UUID from the file stem, infers the MIME from the extension, and emits a
//! `ChatStreamEvent::VideoOut` via the per-turn unbounded channel so the ws
//! send loop can forward it to the browser as a `Message::Binary` frame.
//!
//! Mirrors `WebImageDispatcher` (same directory): the image path delivers
//! generated image bytes as binary `ImageOut` frames; the video path delivers
//! generated video bytes as binary `VideoOut` frames.
//!
//! Key delta from image dispatcher: `VIDEO_SIZE_CAP = 50MB` (not 20MB) — the
//! Telegram `sendVideo` ceiling is larger than the photo ceiling (D-07, Pitfall 4).
//!
//! Do NOT re-persist video bytes here — `video_gen` / `FalClient` already wrote
//! the file to `~/.ironhermes/cache/videos/`. This dispatcher reads only.

#![cfg(feature = "server")]

use std::path::{Path, PathBuf};
use tokio::sync::mpsc::UnboundedSender;

use crate::protocol::ChatStreamEvent;

/// Default video size cap (T-36.3.3-03-01 DoS mitigation; D-07). 50 MB — the
/// Telegram `sendVideo` ceiling. Deliberately larger than the 20 MB photo cap in
/// `web_image_dispatcher`. Do NOT use the photo cap here (Pitfall 4).
///
/// WR-01: this is now only the fallback default. The live cap is threaded in at
/// construction from `config.video_gen.max_inline_bytes` (see `WebVideoDispatcher::new`),
/// so editing that config value actually changes the web dispatcher cap. It is
/// `pub(crate)` so `ws.rs` can fall back to it when the configured value is `0`.
pub(crate) const VIDEO_SIZE_CAP: u64 = 50 * 1024 * 1024;

/// Web-runtime video dispatcher.
///
/// Constructed per-turn in `ws.rs` with a clone of the turn's `tx` sender and
/// the videos cache directory path. On `send_video_file`, reads the video bytes
/// from disk, extracts the UUID from the file stem, infers the MIME from the
/// extension, and emits a `ChatStreamEvent::VideoOut` into the channel.
///
/// The ws send loop's `Message::Binary` arm picks up VideoOut events and
/// forwards them to the browser client as binary WebSocket frames.
pub struct WebVideoDispatcher {
    tx: UnboundedSender<ChatStreamEvent>,
    #[allow(dead_code)]
    // reserved for a future served-path route; binary-frame transport reads only
    videos_cache_dir: PathBuf,
    /// Inline size cap in bytes (WR-01). Threaded in from
    /// `config.video_gen.max_inline_bytes` so the cap is operator-configurable
    /// rather than a duplicate hardcoded const. Videos over this size are skipped.
    size_cap: u64,
}

impl WebVideoDispatcher {
    /// Construct a new dispatcher for a single web turn.
    ///
    /// `tx` — the per-turn unbounded sender for `ChatStreamEvent`; a clone of
    ///        the same `tx` used by the stream/tool callbacks in `ws.rs`.
    /// `videos_cache_dir` — the directory where `video_gen` writes generated
    ///        videos (`get_hermes_home()/cache/videos`).
    /// `size_cap` — the configurable inline cap (WR-01), passed from
    ///        `config.video_gen.max_inline_bytes`. Pass [`VIDEO_SIZE_CAP`] for the
    ///        default 50 MB ceiling.
    pub fn new(
        tx: UnboundedSender<ChatStreamEvent>,
        videos_cache_dir: PathBuf,
        size_cap: u64,
    ) -> Self {
        Self {
            tx,
            videos_cache_dir,
            size_cap,
        }
    }

    /// Read a video from `path`, emit a `ChatStreamEvent::VideoOut` event.
    ///
    /// Extracts the UUID from the file stem (the filename without extension)
    /// and the MIME from the extension. Does NOT re-persist bytes — reads only.
    /// Videos larger than the configured `size_cap` (WR-01:
    /// `config.video_gen.max_inline_bytes`, default [`VIDEO_SIZE_CAP`]) are skipped
    /// (T-36.3.3-03-01) so an oversized video is never binary-framed over the WS.
    pub async fn send_video_file(&self, path: &Path) -> anyhow::Result<()> {
        // Size-gate before reading the whole file into memory (T-36.3.3-03-01).
        let metadata = tokio::fs::metadata(path)
            .await
            .map_err(|e| anyhow::anyhow!("stat video file {}: {e}", path.display()))?;
        if metadata.len() > self.size_cap {
            tracing::warn!(
                path = %path.display(),
                size = metadata.len(),
                cap = self.size_cap,
                "WebVideoDispatcher: video exceeds size cap; skipping VideoOut frame"
            );
            return Ok(());
        }

        // Read video bytes from disk. video_gen/FalClient already persisted;
        // WebVideoDispatcher is a read-only consumer.
        let bytes = tokio::fs::read(path)
            .await
            .map_err(|e| anyhow::anyhow!("read video file {}: {e}", path.display()))?;

        // Extract UUID from file stem (e.g. "abc123" from "abc123.mp4").
        let uuid = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("unknown")
            .to_string();

        let mime = mime_for_video_extension(path);

        let event = ChatStreamEvent::VideoOut { mime, uuid, bytes };

        self.tx
            .send(event)
            .map_err(|e| anyhow::anyhow!("ws send failed: {e}"))?;

        Ok(())
    }
}

/// Infer a video MIME type from a path's extension. Falls back to `video/mp4`.
fn mime_for_video_extension(path: &Path) -> String {
    let ext = path
        .extension()
        .and_then(|s| s.to_str())
        .map(|s| s.to_ascii_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        "mov" => "video/quicktime",
        _ => "video/mp4",
    }
    .to_string()
}

#[cfg(test)]
mod tests {
    //! Behavioral coverage for WebVideoDispatcher.
    //!
    //! Lives here (not in `tests/`) because `iron_hermes_ui` is a binary crate
    //! with no `[lib]` target — integration tests cannot
    //! `use iron_hermes_ui::server::...`. As an in-source unit test it names
    //! the crate-private types directly (mirrors web_image_dispatcher tests).
    use super::{WebVideoDispatcher, VIDEO_SIZE_CAP};
    use crate::protocol::ChatStreamEvent;
    use tempfile::TempDir;
    use tokio::sync::mpsc;

    /// Phase 36.3.3 (D-08): dispatcher reads video bytes, emits VideoOut event
    /// with mime inferred from the extension, uuid=file-stem, bytes=contents.
    #[tokio::test]
    async fn dispatcher_sends_video_out() {
        let (tx, mut rx) = mpsc::unbounded_channel::<ChatStreamEvent>();

        let temp_dir = TempDir::new().expect("create temp dir");
        let uuid = "00000000-0000-0000-0000-000000000001";
        let mp4_path = temp_dir.path().join(format!("{uuid}.mp4"));

        // Small payload to assert byte passthrough.
        let expected_bytes: Vec<u8> = vec![0x00, 0x00, 0x00, 0x18, 0x66, 0x74, 0x79, 0x70];
        std::fs::write(&mp4_path, &expected_bytes).expect("write temp mp4");

        let dispatcher = WebVideoDispatcher::new(tx, temp_dir.path().to_path_buf(), VIDEO_SIZE_CAP);

        let result = dispatcher.send_video_file(&mp4_path).await;
        assert!(result.is_ok(), "send_video_file must return Ok(())");

        let event = rx.recv().await.expect("must receive VideoOut event");
        match event {
            ChatStreamEvent::VideoOut {
                mime,
                uuid: got_uuid,
                bytes,
            } => {
                assert_eq!(mime, "video/mp4", "D-08: mp4 extension → video/mp4");
                assert_eq!(got_uuid, uuid, "D-08: VideoOut uuid must match file stem");
                assert_eq!(
                    bytes, expected_bytes,
                    "D-08: VideoOut bytes must match file contents"
                );
            }
            other => panic!("Expected ChatStreamEvent::VideoOut, got {other:?}"),
        }
    }

    /// T-36.3.3-03-01 / D-07: a video larger than the 50MB cap is skipped (no
    /// frame sent), never binary-framed over the WS. Proves cap is 50MB not 20MB.
    #[tokio::test]
    async fn oversized_video_skipped() {
        let (tx, mut rx) = mpsc::unbounded_channel::<ChatStreamEvent>();

        let temp_dir = TempDir::new().expect("create temp dir");
        let mp4_path = temp_dir.path().join("big.mp4");

        // Write one byte over the 50MB cap.
        let oversized = vec![0u8; (VIDEO_SIZE_CAP + 1) as usize];
        std::fs::write(&mp4_path, &oversized).expect("write oversized mp4");

        let dispatcher = WebVideoDispatcher::new(tx, temp_dir.path().to_path_buf(), VIDEO_SIZE_CAP);

        let result = dispatcher.send_video_file(&mp4_path).await;
        assert!(
            result.is_ok(),
            "oversized video returns Ok (skipped, not error)"
        );
        assert!(
            rx.try_recv().is_err(),
            "T-36.3.3-03-01: no VideoOut frame for a video exceeding the 50MB cap (D-07)"
        );
    }
}
