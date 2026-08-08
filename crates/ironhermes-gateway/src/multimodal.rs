use anyhow::{Result, bail};
use base64::Engine as _;
use std::path::Path;
use std::sync::OnceLock;
use tracing::{info, warn};

use crate::telegram::{TelegramAdapter, TgMessage};
use ironhermes_core::{ChatMessage, ContentPart, ImageUrl, MessageContent, Role};

/// Maximum file size accepted for download (20MB per D-08).
const MAX_FILE_SIZE: i64 = 20 * 1024 * 1024;

/// Maximum per-image payload actually SENT to a vision provider. Images at or
/// below this pass through [`fit_image_for_vision`] untouched; larger images
/// are downscaled/re-encoded until the result fits (or dropped if it cannot).
/// Keeps every sent payload within provider per-image limits (Anthropic ~5 MB)
/// and reduces latency.
pub const IMAGE_SEND_MAX_BYTES: usize = 4 * 1024 * 1024;

/// Target maximum long-edge (px) when downscaling an oversized image. Anthropic
/// recommends a long edge <= 1568 px for best results and to avoid server-side
/// resizing latency (46.5.1).
pub const IMAGE_MAX_EDGE: u32 = 1568;

/// Smallest long-edge (px) [`fit_image_for_vision`] will downscale to while
/// trying to fit under [`IMAGE_SEND_MAX_BYTES`]. Below this the image is too
/// degraded to be useful vision input, so it is dropped instead — callers then
/// fail loud rather than silently omitting it.
pub const IMAGE_MIN_EDGE: u32 = 256;

/// JPEG quality used when the lossless PNG re-encode of a candidate size is
/// still over the send ceiling (photographic content compresses poorly as PNG).
const IMAGE_JPEG_QUALITY: u8 = 85;

/// Ensure an image's payload is small enough to actually reach the vision
/// model. Images at or under [`IMAGE_SEND_MAX_BYTES`] pass through unchanged;
/// every payload this returns fits under that ceiling, so it never exceeds
/// provider per-image limits (Anthropic ~5 MB).
///
/// Larger images are decoded (the `image` crate's default `Limits` cap decoder
/// allocation at 512 MiB, so crafted dimension bombs fail cleanly instead of
/// exhausting memory) and re-encoded at a shrinking long-edge target: starting
/// from [`IMAGE_MAX_EDGE`] (clamped to at least [`IMAGE_MIN_EDGE`] so even a
/// tiny-dimension image bloated past the ceiling by metadata gets a native-size
/// re-encode) and halving until something fits. At each size a lossless PNG is
/// tried first (best for screenshots/diagrams); if that is over the ceiling, a
/// JPEG at quality [`IMAGE_JPEG_QUALITY`] is tried before shrinking further, so
/// photographic content keeps its resolution instead of being halved (a photo's
/// PNG at 1568px is often > 4 MiB while its JPEG is well under 1 MiB).
///
/// Returns `None` only when the bytes cannot be decoded as an image or nothing
/// fits by [`IMAGE_MIN_EDGE`] — callers should treat the attachment as
/// undisplayable and fail loud rather than silently dropping it.
pub fn fit_image_for_vision(bytes: &[u8], mime: &str) -> Option<(Vec<u8>, String)> {
    // Small enough to send as-is.
    if bytes.len() <= IMAGE_SEND_MAX_BYTES {
        return Some((bytes.to_vec(), mime.to_string()));
    }

    let decoded = image::load_from_memory(bytes).ok()?;

    let long_edge = decoded.width().max(decoded.height());
    let mut edge = IMAGE_MAX_EDGE.min(long_edge).max(IMAGE_MIN_EDGE);
    while edge >= IMAGE_MIN_EDGE {
        // `resize` fits within the box, preserving aspect ratio. Borrow the
        // original when it already fits the target (no clone of the bitmap).
        let resized;
        let candidate: &image::DynamicImage = if long_edge <= edge {
            &decoded
        } else {
            resized = decoded.resize(edge, edge, image::imageops::FilterType::Triangle);
            &resized
        };

        let mut png = std::io::Cursor::new(Vec::new());
        candidate.write_to(&mut png, image::ImageFormat::Png).ok()?;
        let png = png.into_inner();
        if png.len() <= IMAGE_SEND_MAX_BYTES {
            return Some((png, "image/png".to_string()));
        }

        // PNG too big at this size — try JPEG before shrinking (JPEG has no
        // alpha, so flatten to RGB first).
        let mut jpg = std::io::Cursor::new(Vec::new());
        let ok = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpg, IMAGE_JPEG_QUALITY)
            .encode_image(&candidate.to_rgb8())
            .is_ok();
        if ok {
            let jpg = jpg.into_inner();
            if jpg.len() <= IMAGE_SEND_MAX_BYTES {
                return Some((jpg, "image/jpeg".to_string()));
            }
        }

        edge /= 2;
    }
    None
}

// =============================================================================
// Surface-agnostic local attachment pipeline (D-01/D-12, Phase 46.7 Plan 02)
// =============================================================================
//
// `process_local_attachment` + `build_chat_user_message` below are the ONE
// shared attachment-to-model-turn delivery pipeline both the web-chat
// (Plan 04) and TUI (Plan 06) surfaces feed. Unlike `process_attachments`
// above (which is Telegram-specific — it takes a `TelegramAdapter` and
// downloads bytes itself), these take raw bytes directly so any surface can
// call them after acquiring bytes its own way. They reuse `fit_image_for_vision`
// and `pdf_extract::extract_text_from_mem` rather than re-deriving them, per
// the RESEARCH anti-pattern flag against duplicate downscale logic.

/// Maximum size (bytes) of extracted/decoded text before a text or code
/// attachment is inlined verbatim into the user turn (D-02). Chosen as a
/// "tens of KB" anchor per RESEARCH assumption A1 — large enough to inline a
/// typical source file or short document, small enough to avoid blowing the
/// context window on a single attachment.
pub const INLINE_TEXT_MAX_BYTES: usize = 32 * 1024;

/// Maximum accepted size (bytes) for a non-image attachment (PDF, text,
/// code). Rejected before any decode/extract to bound decoder/extractor work
/// (D-04, T-46.7-04). Matches the kanban `CLIENT_MAX_ATTACHMENT_BYTES`
/// convention and the UI-SPEC copy (RESEARCH assumption A2). Images are NOT
/// bound by this constant — they ride [`fit_image_for_vision`]'s own
/// decoder-allocation ceiling instead.
pub const NONIMAGE_MAX_BYTES: usize = 10 * 1024 * 1024;

/// Maximum number of attachments processed into a single chat turn (RESEARCH
/// assumption A3). Attachments beyond this are dropped with a surplus note
/// in [`build_chat_user_message`] rather than silently unbounded (D-04).
pub const MAX_ATTACHMENTS_PER_MESSAGE: usize = 10;

/// How a PDF attachment is turned into model input (D-03).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PdfMode {
    /// Extract text via `pdf_extract` — the default, always-available mode.
    #[default]
    TextExtract,
    /// Rasterize the first page to an image via the external `pdftoppm`
    /// binary and deliver it through the vision path. Degrades to
    /// [`PdfMode::TextExtract`] when the binary is not present on the host
    /// (T-46.7-SC — no new crate dependency).
    Rasterize,
}

/// A single local attachment, classified and ready for turn assembly via
/// [`build_chat_user_message`] (D-01/D-12) — the surface-agnostic sibling of
/// Telegram's [`ProcessedAttachments`], shared by both the web-chat and TUI
/// surfaces.
#[derive(Debug, Clone)]
pub enum LocalAttachment {
    /// An image (or a rasterized PDF page), already fitted for vision
    /// delivery via [`fit_image_for_vision`] and base64-encoded as a
    /// `data:<mime>;base64,...` URI.
    Image { data_uri: String },
    /// Text/code (or PDF-extracted text) small enough to inline verbatim
    /// into the user turn as a fenced block (D-02).
    InlineText { header: String, body: String },
    /// Text/code too large to inline; deferred to the session workspace
    /// (D-02/D-23) — only a reference note is placed in the user turn, the
    /// body is never inlined.
    WorkspaceFile { filename: String, note: String },
}

/// Classify and process a raw local attachment (D-01) into a
/// [`LocalAttachment`] ready for turn assembly via [`build_chat_user_message`].
/// The surface-agnostic sibling of [`process_attachments`] — takes raw bytes
/// directly rather than a [`TelegramAdapter`], so both the web-chat and TUI
/// surfaces can reuse it (D-12).
///
/// - Images: fitted for vision via [`fit_image_for_vision`] (no duplicate
///   downscale logic) and base64-encoded; an undecodable image falls back to
///   raw bytes with a warn, mirroring [`process_attachments`]'s photo path.
/// - PDFs: text extracted via `pdf_extract` by default (D-03); pass
///   [`PdfMode::Rasterize`] for the first-page-image alternative mode, which
///   degrades to text extraction when the external rasterizer is absent.
/// - Text/code: read as UTF-8 (lossy) and classified as inline or
///   workspace-deferred by [`INLINE_TEXT_MAX_BYTES`] (D-02).
///
/// Non-image attachments over [`NONIMAGE_MAX_BYTES`] are rejected before any
/// decode/extract (T-46.7-04); images ride [`fit_image_for_vision`]'s own
/// ceiling instead.
pub fn process_local_attachment(
    bytes: &[u8],
    mime: &str,
    filename: &str,
    mode: PdfMode,
) -> Result<LocalAttachment> {
    if mime.starts_with("image/") {
        return Ok(process_image(bytes, mime));
    }

    if bytes.len() > NONIMAGE_MAX_BYTES {
        bail!(
            "Attachment {} exceeds the maximum accepted size ({} bytes > {} byte cap).",
            filename,
            bytes.len(),
            NONIMAGE_MAX_BYTES
        );
    }

    let is_pdf = mime == "application/pdf" || filename.to_ascii_lowercase().ends_with(".pdf");
    if is_pdf {
        return match mode {
            PdfMode::TextExtract => process_pdf_text_extract(bytes, filename),
            PdfMode::Rasterize => {
                process_pdf_rasterize(bytes, filename, pdf_rasterizer_available())
            }
        };
    }

    Ok(process_text(bytes, filename))
}

/// Fit + base64-encode an image attachment, mirroring [`process_attachments`]'s
/// Telegram photo path exactly (same fit-then-fallback-to-raw behavior).
fn process_image(bytes: &[u8], mime: &str) -> LocalAttachment {
    let (send_bytes, send_mime) = match fit_image_for_vision(bytes, mime) {
        Some(fitted) => fitted,
        None => {
            warn!(
                size_bytes = bytes.len(),
                "attachment image could not be decoded for vision fitting; sending raw bytes as-is"
            );
            (bytes.to_vec(), mime.to_string())
        }
    };
    let encoded = base64::engine::general_purpose::STANDARD.encode(&send_bytes);
    LocalAttachment::Image {
        data_uri: format!("data:{};base64,{}", send_mime, encoded),
    }
}

/// Extract PDF text via `pdf_extract`, wrap with a `[Document: ...]`-style
/// header (mirrors [`process_attachments`]'s document path), and classify by
/// [`INLINE_TEXT_MAX_BYTES`].
fn process_pdf_text_extract(bytes: &[u8], filename: &str) -> Result<LocalAttachment> {
    let text = match pdf_extract::extract_text_from_mem(bytes) {
        Ok(extracted) => extracted,
        Err(e) => {
            warn!(error = %e, filename, "PDF text extraction failed");
            bail!("Could not extract text from PDF {}: {}", filename, e);
        }
    };
    Ok(classify_text(
        filename,
        format!("[Document: {}]", filename),
        text,
    ))
}

/// Read text/code bytes as UTF-8 (lossy) and classify by
/// [`INLINE_TEXT_MAX_BYTES`] (D-02).
fn process_text(bytes: &[u8], filename: &str) -> LocalAttachment {
    let text = String::from_utf8_lossy(bytes).into_owned();
    classify_text(filename, format!("File: {}", filename), text)
}

/// Shared inline-vs-workspace classification used by both
/// [`process_pdf_text_extract`] and [`process_text`] (D-02).
fn classify_text(filename: &str, header: String, body: String) -> LocalAttachment {
    if body.len() <= INLINE_TEXT_MAX_BYTES {
        LocalAttachment::InlineText { header, body }
    } else {
        LocalAttachment::WorkspaceFile {
            filename: filename.to_string(),
            note: format!(
                "exceeds the {}-byte inline threshold ({} bytes); deferred to the session workspace",
                INLINE_TEXT_MAX_BYTES,
                body.len()
            ),
        }
    }
}

/// Process-cached probe for whether the external `pdftoppm` (poppler-utils)
/// binary is present on this host (D-03 alternative mode; T-46.7-SC — no new
/// crate, mirrors the `ffmpeg_available` graceful-degrade convention in
/// `ironhermes-tools/src/tts/mod.rs`).
static PDF_RASTERIZER_AVAILABLE: OnceLock<bool> = OnceLock::new();

/// Returns `true` if `pdftoppm` is present and runs successfully on this
/// host. Result is cached in a `OnceLock` — the probe runs at most once per
/// process. Safe to call from any thread; never panics on any platform.
pub fn pdf_rasterizer_available() -> bool {
    *PDF_RASTERIZER_AVAILABLE.get_or_init(|| {
        std::process::Command::new("pdftoppm")
            .arg("-v")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    })
}

/// Rasterize a PDF's first page to an image via `pdftoppm`, degrading to
/// [`process_pdf_text_extract`] when the rasterizer is unavailable
/// (T-46.7-SC — a runtime availability degrade, not a scope reduction of
/// D-03). Split out from [`process_local_attachment`]'s PDF branch so tests
/// can force the `available` flag deterministically without depending on the
/// host actually having poppler-utils installed.
fn process_pdf_rasterize(bytes: &[u8], filename: &str, available: bool) -> Result<LocalAttachment> {
    if !available {
        warn!(
            filename,
            "pdftoppm not available; PDF rasterization mode degraded to text extraction"
        );
        return process_pdf_text_extract(bytes, filename);
    }

    let png_bytes = rasterize_pdf_first_page(bytes)?;
    Ok(process_image(&png_bytes, "image/png"))
}

/// Shell out to `pdftoppm` to render a PDF's first page to PNG bytes.
/// First-page-only in v1 to bound the rasterization work and contain the
/// decoder/dimension-bomb risk (T-46.7-04); the resulting PNG is re-fitted
/// through [`fit_image_for_vision`]'s existing ceiling by the caller.
fn rasterize_pdf_first_page(bytes: &[u8]) -> Result<Vec<u8>> {
    let dir = tempfile::tempdir()?;
    let input_path = dir.path().join("input.pdf");
    std::fs::write(&input_path, bytes)?;
    let output_prefix = dir.path().join("page");

    let status = std::process::Command::new("pdftoppm")
        .args(["-png", "-f", "1", "-l", "1", "-singlefile"])
        .arg(&input_path)
        .arg(&output_prefix)
        .status()?;
    if !status.success() {
        bail!("pdftoppm exited with status {}", status);
    }

    let output_path = dir.path().join("page.png");
    std::fs::read(&output_path)
        .map_err(|e| anyhow::anyhow!("pdftoppm did not produce an output PNG: {}", e))
}

/// Assemble a hybrid inline/workspace/image user turn from local attachments
/// (D-12) — the single delivery entry point both the web-chat (Plan 04) and
/// TUI (Plan 06) surfaces call, so they produce identical model input from
/// the same attachment bytes. Mirrors `handler.rs`'s `build_user_message`
/// `Parts`/`ImageUrl` branching exactly.
///
/// - Image attachments become one [`ContentPart::ImageUrl`] each; if any
///   images are present the result is `MessageContent::Parts`, otherwise
///   plain `MessageContent::Text` (D-07 supports an image-only/no-caption
///   turn either way).
/// - `InlineText` attachments are appended to the caption as a fenced block
///   with a filename header.
/// - `WorkspaceFile` attachments append a one-line note pointing at
///   `workspace_path.join(filename)` instead of inlining the body (D-02).
/// - Attachments beyond [`MAX_ATTACHMENTS_PER_MESSAGE`] are dropped with a
///   surplus note rather than silently unbounded (D-04).
pub fn build_chat_user_message(
    text: &str,
    attachments: &[LocalAttachment],
    workspace_path: &Path,
) -> ChatMessage {
    let mut caption = text.to_string();
    let mut image_parts = Vec::new();

    let take = attachments.len().min(MAX_ATTACHMENTS_PER_MESSAGE);
    for attachment in &attachments[..take] {
        match attachment {
            LocalAttachment::Image { data_uri } => {
                image_parts.push(ContentPart::ImageUrl {
                    image_url: ImageUrl {
                        url: data_uri.clone(),
                        detail: None,
                    },
                });
            }
            LocalAttachment::InlineText { header, body } => {
                append_section(&mut caption, &format!("{}\n```\n{}\n```", header, body));
            }
            LocalAttachment::WorkspaceFile { filename, .. } => {
                append_section(
                    &mut caption,
                    &format!(
                        "[Attachment saved at {} — read it with your file tools.]",
                        workspace_path.join(filename).display()
                    ),
                );
            }
        }
    }

    if attachments.len() > MAX_ATTACHMENTS_PER_MESSAGE {
        let surplus = attachments.len() - MAX_ATTACHMENTS_PER_MESSAGE;
        append_section(
            &mut caption,
            &format!(
                "[{} additional attachment(s) exceeded the {}-attachment-per-message limit and were dropped.]",
                surplus, MAX_ATTACHMENTS_PER_MESSAGE
            ),
        );
    }

    let content = if image_parts.is_empty() {
        MessageContent::text(caption)
    } else {
        let mut parts = vec![ContentPart::Text { text: caption }];
        parts.extend(image_parts);
        MessageContent::Parts(parts)
    };

    ChatMessage {
        role: Role::User,
        content: Some(content),
        tool_calls: None,
        tool_call_id: None,
        name: None,
        is_recall_context: false,
    }
}

/// Append `section` to `caption`, separating with a blank line when `caption`
/// already has content (so an empty caption + one attachment note still
/// reads cleanly, per D-07).
fn append_section(caption: &mut String, section: &str) {
    if !caption.is_empty() {
        caption.push_str("\n\n");
    }
    caption.push_str(section);
}

/// Result of processing a TgMessage's attachments.
pub struct ProcessedAttachments {
    /// Text to prepend to the user message (extracted document content).
    pub text_prefix: Option<String>,
    /// Base64-encoded image data URI for vision models (lets the model SEE the image).
    pub image_data_uri: Option<String>,
    /// Absolute path of the attached image written to the local image cache, if any.
    ///
    /// Vision data URIs cannot be consumed by `video_animate` (it base64-encodes a
    /// file PATH or fetches a public URL — never inline `data:` data). We persist the
    /// inbound photo under `get_hermes_home()/cache/images/` so the model can pass this
    /// short PATH to `video_animate` for image-to-video. `None` when there is no image
    /// or the cache write failed (the vision data URI is still returned in that case).
    pub image_cache_path: Option<String>,
}

/// Process any photo or document attached to `msg`.
///
/// - Photos: downloaded and base64-encoded as a `data:image/jpeg;base64,...` URI.
/// - PDF documents: text extracted via `pdf_extract` and returned as a text prefix.
/// - Plain-text documents: read as UTF-8 and returned as a text prefix.
/// - Oversized files (>20MB): error returned to caller for user-facing message.
pub async fn process_attachments(
    adapter: &TelegramAdapter,
    msg: &TgMessage,
) -> Result<ProcessedAttachments> {
    // --- Photo ---
    if let Some(ref photos) = msg.photo
        && let Some(largest) = photos.last()
    {
        // Check file size if known
        if let Some(size) = largest.file_size
            && size > MAX_FILE_SIZE
        {
            bail!("File too large (max 20MB). Please send a smaller image.");
        }

        info!(file_id = %largest.file_id, "Downloading photo from Telegram");
        let tg_file = adapter.get_file(&largest.file_id).await?;
        let file_path = tg_file
            .file_path
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("Telegram returned no file_path for photo"))?;

        let bytes = adapter.download_file(file_path).await?;

        // Fit the payload under the provider per-image limit before it is
        // embedded in the request. Telegram photos pass the 20MB accept gate
        // above, but providers reject images over ~5 MB at request time — the
        // same failure 46.5.1 fixed on the kanban worker path. Fall back to the
        // raw bytes only if the image cannot be decoded at all (rare for
        // Telegram-transcoded JPEGs; matches the old behavior in that case).
        let (send_bytes, send_mime) = match fit_image_for_vision(&bytes, "image/jpeg") {
            Some(fitted) => fitted,
            None => {
                warn!(
                    size_bytes = bytes.len(),
                    "photo could not be decoded for vision fitting; sending raw bytes as-is"
                );
                (bytes.clone(), "image/jpeg".to_string())
            }
        };
        let encoded = base64::engine::general_purpose::STANDARD.encode(&send_bytes);
        let data_uri = format!("data:{};base64,{}", send_mime, encoded);

        // Also persist the photo to the local image cache so the model can pass a short
        // PATH to `video_animate` (image-to-video). The vision `data_uri` above lets the
        // model SEE the image; the cache PATH lets it ANIMATE the image. Cache-write
        // failure is non-fatal — we still return the data URI for vision.
        let image_cache_path = match write_image_to_cache(&bytes) {
            Ok(path) => Some(path),
            Err(e) => {
                warn!(error = %e, "Failed to write inbound photo to image cache; image-to-video will be unavailable for this message");
                None
            }
        };

        return Ok(ProcessedAttachments {
            image_data_uri: Some(data_uri),
            image_cache_path,
            text_prefix: None,
        });
    }

    // --- Document ---
    if let Some(ref doc) = msg.document {
        if let Some(size) = doc.file_size
            && size > MAX_FILE_SIZE
        {
            bail!("File too large (max 20MB). Please send a smaller file.");
        }

        let mime = doc
            .mime_type
            .as_deref()
            .unwrap_or("application/octet-stream");
        let filename = doc.file_name.as_deref().unwrap_or("document");

        info!(file_id = %doc.file_id, mime_type = %mime, filename = %filename, "Downloading document from Telegram");
        let tg_file = adapter.get_file(&doc.file_id).await?;
        let file_path = tg_file
            .file_path
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("Telegram returned no file_path for document"))?;

        let bytes = adapter.download_file(file_path).await?;

        let text = match mime {
            "application/pdf" => match pdf_extract::extract_text_from_mem(&bytes) {
                Ok(extracted) => extracted,
                Err(e) => {
                    warn!(error = %e, "PDF text extraction failed, returning error");
                    bail!("Could not extract text from PDF: {}", e);
                }
            },
            "text/plain" | "text/markdown" | "text/csv" | "text/html" => {
                String::from_utf8_lossy(&bytes).into_owned()
            }
            other => {
                bail!(
                    "Unsupported document type: {}. Supported types: PDF, plain text, markdown, CSV.",
                    other
                );
            }
        };

        let wrapped = format!("[Document: {}]\n{}", filename, text);
        return Ok(ProcessedAttachments {
            text_prefix: Some(wrapped),
            image_data_uri: None,
            image_cache_path: None,
        });
    }

    // No attachment
    Ok(ProcessedAttachments {
        text_prefix: None,
        image_data_uri: None,
        image_cache_path: None,
    })
}

/// Build the unique cache filename for an inbound image.
///
/// Mirrors the convention in `ironhermes-tools/src/fal.rs` (`cache_image_path`):
/// `<UTC %Y%m%dT%H%M%S>-<uuid v4>.jpg`. Telegram delivers photos as JPEG, so the
/// extension is fixed at `.jpg`. Pure and unique — extracted for unit testing.
fn image_cache_filename() -> String {
    format!(
        "{}-{}.jpg",
        chrono::Utc::now().format("%Y%m%dT%H%M%S"),
        uuid::Uuid::new_v4()
    )
}

/// Write `bytes` to `get_hermes_home()/cache/images/<filename>` and return the
/// absolute path as a String.
///
/// The file MUST land under `get_hermes_home()/cache` so that
/// `video_gen::resolve_image_url`'s cache-root clamp accepts it. Creates the
/// directory if needed.
fn write_image_to_cache(bytes: &[u8]) -> Result<String> {
    write_image_to_cache_in(&ironhermes_core::constants::get_hermes_home(), bytes)
}

/// Inner, dependency-injected variant of [`write_image_to_cache`] for testability.
///
/// Writes to `home/cache/images/<filename>`; `home` is the hermes home directory.
fn write_image_to_cache_in(home: &std::path::Path, bytes: &[u8]) -> Result<String> {
    let dir = home.join("cache").join("images");
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(image_cache_filename());
    std::fs::write(&path, bytes)?;
    Ok(path.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `side`×`side` PNG whose per-pixel values are pseudo-random (splitmix64)
    /// so PNG cannot compress it away — at side²×3 raw bytes the encoded size
    /// reliably clears the send ceiling.
    fn incompressible_png(side: u32) -> Vec<u8> {
        let img = image::RgbImage::from_fn(side, side, |x, y| {
            let mut v = (x as u64)
                .wrapping_mul(0x9E37_79B9_7F4A_7C15)
                .wrapping_add((y as u64).wrapping_mul(0xD1B5_4A32_D192_ED03));
            v ^= v >> 29;
            v = v.wrapping_mul(0xBF58_476D_1CE4_E5B9);
            v ^= v >> 32;
            image::Rgb([
                (v & 0xFF) as u8,
                ((v >> 8) & 0xFF) as u8,
                ((v >> 16) & 0xFF) as u8,
            ])
        });
        let mut out = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(img)
            .write_to(&mut out, image::ImageFormat::Png)
            .expect("encode test png");
        out.into_inner()
    }

    #[test]
    fn fit_image_small_payload_passes_through_untouched() {
        let small = incompressible_png(64);
        assert!(small.len() <= IMAGE_SEND_MAX_BYTES);
        let (payload, mime) = fit_image_for_vision(&small, "image/png").unwrap();
        assert_eq!(
            payload, small,
            "small payloads must pass through byte-identical"
        );
        assert_eq!(mime, "image/png");
    }

    #[test]
    fn fit_image_oversized_payload_fits_under_send_ceiling() {
        // > IMAGE_MAX_EDGE dims and > ceiling bytes: must come back fitted.
        let big = incompressible_png(1800); // 1800*1800*3 ≈ 9.3 MiB raw
        assert!(
            big.len() > IMAGE_SEND_MAX_BYTES,
            "fixture too small: {}",
            big.len()
        );
        let (payload, mime) = fit_image_for_vision(&big, "image/png").unwrap();
        assert!(
            payload.len() <= IMAGE_SEND_MAX_BYTES,
            "payload must be under the send ceiling; got {}",
            payload.len()
        );
        assert!(
            mime == "image/png" || mime == "image/jpeg",
            "unexpected mime {mime}"
        );
    }

    #[test]
    fn fit_image_dense_in_envelope_image_is_reencoded_not_passed_through() {
        // Dims already <= IMAGE_MAX_EDGE but bytes over the ceiling: the old
        // single-pass version sent this as-is (up to 20 MiB) and the provider
        // rejected it at request time.
        let dense = incompressible_png(1400); // 1400*1400*3 ≈ 5.6 MiB raw
        assert!(dense.len() > IMAGE_SEND_MAX_BYTES);
        let (payload, _mime) = fit_image_for_vision(&dense, "image/png").unwrap();
        assert!(
            payload.len() <= IMAGE_SEND_MAX_BYTES,
            "dense image must be re-encoded under the ceiling; got {}",
            payload.len()
        );
    }

    #[test]
    fn fit_image_huge_raw_input_is_downscaled_not_dropped() {
        // Regression: a former pre-decode 20 MiB gate dropped legitimately huge
        // images outright; they must be decoded and downscaled instead.
        let huge = incompressible_png(2700); // 2700*2700*3 ≈ 20.9 MiB raw
        assert!(
            huge.len() > 20 * 1024 * 1024,
            "fixture too small: {}",
            huge.len()
        );
        let (payload, _mime) = fit_image_for_vision(&huge, "image/png").unwrap();
        assert!(payload.len() <= IMAGE_SEND_MAX_BYTES);
    }

    #[test]
    fn fit_image_tiny_dims_metadata_bloated_image_gets_native_reencode() {
        // Regression: a decodable image whose long edge is under IMAGE_MIN_EDGE
        // but whose byte size clears the ceiling (e.g. multi-MB ancillary
        // chunks) used to skip the fit loop entirely and be dropped. PNG decode
        // ignores trailing bytes after IEND, so junk-padding a tiny image
        // models the bloat.
        let mut bloated = incompressible_png(100);
        bloated.resize(IMAGE_SEND_MAX_BYTES + 1, 0);
        let (payload, mime) = fit_image_for_vision(&bloated, "image/png")
            .expect("tiny-dims bloated image must be re-encoded at native size, not dropped");
        assert!(payload.len() <= IMAGE_SEND_MAX_BYTES);
        assert_eq!(mime, "image/png");
    }

    #[test]
    fn fit_image_undecodable_oversized_returns_none() {
        let junk = vec![0u8; IMAGE_SEND_MAX_BYTES + 1];
        assert!(fit_image_for_vision(&junk, "image/png").is_none());
    }

    #[test]
    fn fit_image_photographic_content_keeps_resolution_via_jpeg() {
        // A noisy image whose PNG at IMAGE_MAX_EDGE exceeds the ceiling must
        // fall back to JPEG at the SAME edge rather than halving resolution.
        let big = incompressible_png(1800);
        let (payload, mime) = fit_image_for_vision(&big, "image/png").unwrap();
        if mime == "image/jpeg" {
            let dims = image::load_from_memory(&payload).unwrap();
            assert_eq!(
                dims.width().max(dims.height()),
                IMAGE_MAX_EDGE,
                "JPEG fallback should preserve the full target edge"
            );
        }
        assert!(payload.len() <= IMAGE_SEND_MAX_BYTES);
    }

    #[test]
    fn image_cache_filename_has_jpg_extension_and_is_unique() {
        let a = image_cache_filename();
        let b = image_cache_filename();
        assert!(a.ends_with(".jpg"), "expected .jpg extension, got {a}");
        // UUID v4 component guarantees uniqueness even within the same second.
        assert_ne!(a, b, "two filenames in a row should differ (uuid v4)");
    }

    #[test]
    fn write_image_to_cache_lands_under_cache_root() {
        let tmp = std::env::temp_dir().join(format!("ironhermes-mm-test-{}", uuid::Uuid::new_v4()));
        let path =
            write_image_to_cache_in(&tmp, b"fake-jpeg-bytes").expect("cache write should succeed");
        let cache_root = tmp.join("cache");
        assert!(
            std::path::Path::new(&path).starts_with(&cache_root),
            "written path {path} must be under cache root {}",
            cache_root.display()
        );
        assert!(
            std::path::Path::new(&path).exists(),
            "file should exist on disk"
        );
        let _ = std::fs::remove_dir_all(&tmp);
    }

    // -------------------------------------------------------------------
    // process_local_attachment (Task 1, D-01/D-04)
    // -------------------------------------------------------------------

    /// Build a minimal, well-formed single-page PDF containing `text` as a
    /// `Tj` text-showing operator, with a correct byte-offset xref table so
    /// `pdf_extract::extract_text_from_mem` can actually parse it. Hand-built
    /// rather than a binary fixture so the test has no external file deps.
    fn minimal_text_pdf(text: &str) -> Vec<u8> {
        let mut buf: Vec<u8> = Vec::new();
        let mut offsets: Vec<usize> = Vec::new();

        let mut push_obj = |buf: &mut Vec<u8>, s: String| {
            offsets.push(buf.len());
            buf.extend_from_slice(s.as_bytes());
        };

        buf.extend_from_slice(b"%PDF-1.4\n");
        push_obj(
            &mut buf,
            "1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n".to_string(),
        );
        push_obj(
            &mut buf,
            "2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 >>\nendobj\n".to_string(),
        );
        push_obj(
            &mut buf,
            "3 0 obj\n<< /Type /Page /Parent 2 0 R /Resources << /Font << /F1 4 0 R >> >> \
             /MediaBox [0 0 200 200] /Contents 5 0 R >>\nendobj\n"
                .to_string(),
        );
        push_obj(
            &mut buf,
            "4 0 obj\n<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>\nendobj\n".to_string(),
        );
        let content = format!("BT /F1 24 Tf 10 100 Td ({}) Tj ET", text);
        push_obj(
            &mut buf,
            format!(
                "5 0 obj\n<< /Length {} >>\nstream\n{}\nendstream\nendobj\n",
                content.len(),
                content
            ),
        );

        let xref_offset = buf.len();
        let mut xref = String::from("xref\n0 6\n0000000000 65535 f \n");
        for off in &offsets {
            xref.push_str(&format!("{:010} 00000 n \n", off));
        }
        buf.extend_from_slice(xref.as_bytes());
        buf.extend_from_slice(
            format!(
                "trailer\n<< /Size 6 /Root 1 0 R >>\nstartxref\n{}\n%%EOF",
                xref_offset
            )
            .as_bytes(),
        );

        buf
    }

    #[test]
    fn process_local_attachment_image_returns_data_uri() {
        let png = incompressible_png(64);
        let result = process_local_attachment(&png, "image/png", "a.png", PdfMode::TextExtract)
            .expect("image attachment should classify successfully");
        match result {
            LocalAttachment::Image { data_uri } => {
                assert!(
                    data_uri.starts_with("data:image/"),
                    "expected a data:image/ URI, got {data_uri}"
                );
            }
            other => panic!("expected LocalAttachment::Image, got {other:?}"),
        }
    }

    #[test]
    fn process_local_attachment_undecodable_oversized_image_falls_back_to_raw() {
        // Mirrors fit_image_undecodable_oversized_returns_none: junk bytes over
        // the send ceiling cannot be decoded, so fit_image_for_vision returns
        // None and process_local_attachment must fall back to raw bytes rather
        // than panicking or erroring.
        let junk = vec![0u8; IMAGE_SEND_MAX_BYTES + 1];
        let result = process_local_attachment(&junk, "image/png", "bad.png", PdfMode::TextExtract)
            .expect("undecodable image must fall back to raw bytes, not error");
        match result {
            LocalAttachment::Image { data_uri } => {
                assert!(data_uri.starts_with("data:image/png;base64,"));
            }
            other => panic!("expected LocalAttachment::Image fallback, got {other:?}"),
        }
    }

    #[test]
    fn process_local_attachment_pdf_returns_text_variant_with_document_header() {
        let pdf = minimal_text_pdf("Hello World");
        let result =
            process_local_attachment(&pdf, "application/pdf", "d.pdf", PdfMode::TextExtract)
                .expect("pdf text extraction should succeed on a well-formed PDF");
        match result {
            LocalAttachment::InlineText { header, body } => {
                assert_eq!(header, "[Document: d.pdf]");
                assert!(
                    body.contains("Hello World"),
                    "expected extracted text to contain the PDF's content, got: {body}"
                );
            }
            LocalAttachment::WorkspaceFile { filename, .. } => {
                assert_eq!(filename, "d.pdf");
            }
            other => panic!("expected a text-carrying variant, got {other:?}"),
        }
    }

    #[test]
    fn process_local_attachment_small_text_returns_inline_text() {
        let result = process_local_attachment(
            b"fn main() {}\n",
            "text/plain",
            "n.txt",
            PdfMode::TextExtract,
        )
        .expect("small text attachment should classify successfully");
        match result {
            LocalAttachment::InlineText { header, body } => {
                assert!(
                    header.contains("n.txt"),
                    "header should name the file: {header}"
                );
                assert_eq!(body, "fn main() {}\n");
            }
            other => panic!("expected LocalAttachment::InlineText, got {other:?}"),
        }
    }

    #[test]
    fn process_local_attachment_oversize_text_returns_workspace_file() {
        let big_text = "x".repeat(INLINE_TEXT_MAX_BYTES + 1);
        let result = process_local_attachment(
            big_text.as_bytes(),
            "text/plain",
            "big.txt",
            PdfMode::TextExtract,
        )
        .expect("oversize text should classify (not error) as WorkspaceFile");
        match result {
            LocalAttachment::WorkspaceFile { filename, .. } => {
                assert_eq!(filename, "big.txt");
            }
            other => panic!("expected LocalAttachment::WorkspaceFile, got {other:?}"),
        }
    }

    #[test]
    fn process_local_attachment_nonimage_over_cap_returns_err() {
        let huge = vec![b'x'; NONIMAGE_MAX_BYTES + 1];
        let result =
            process_local_attachment(&huge, "text/plain", "huge.txt", PdfMode::TextExtract);
        assert!(
            result.is_err(),
            "non-image attachment over NONIMAGE_MAX_BYTES must be rejected before decode"
        );
    }

    // -------------------------------------------------------------------
    // PDF rasterization alt-mode (Task 2, D-03)
    // -------------------------------------------------------------------

    #[test]
    fn pdf_rasterize_degrades_to_text_extraction_when_unavailable() {
        let pdf = minimal_text_pdf("Scanned Doc");
        let result = process_pdf_rasterize(&pdf, "scan.pdf", false)
            .expect("rasterizer-unavailable degrade must not error");
        match result {
            LocalAttachment::InlineText { .. } | LocalAttachment::WorkspaceFile { .. } => {}
            other => panic!("expected a degraded text-carrying variant, got {other:?}"),
        }
    }

    #[test]
    fn pdf_rasterize_produces_image_when_available() {
        // Real-binary-dependent: skip gracefully on hosts without poppler-utils,
        // matching the project's ffmpeg-absent-degrade convention. The
        // deterministic `available=false` degrade path is covered above.
        if !pdf_rasterizer_available() {
            eprintln!("pdftoppm not present on this host; skipping rasterize-available test");
            return;
        }
        let pdf = minimal_text_pdf("Scanned Doc");
        let result = process_pdf_rasterize(&pdf, "scan.pdf", true)
            .expect("rasterization should succeed when pdftoppm is available");
        match result {
            LocalAttachment::Image { data_uri } => {
                assert!(data_uri.starts_with("data:image/"));
            }
            other => panic!("expected LocalAttachment::Image, got {other:?}"),
        }
    }

    // -------------------------------------------------------------------
    // build_chat_user_message (Task 3, D-02/D-07/D-12)
    // -------------------------------------------------------------------

    #[test]
    fn build_chat_user_message_image_and_text_yields_one_image_url_per_image() {
        let png = incompressible_png(64);
        let image = process_local_attachment(&png, "image/png", "a.png", PdfMode::TextExtract)
            .expect("image should classify");
        let inline = LocalAttachment::InlineText {
            header: "File: n.txt".to_string(),
            body: "hello".to_string(),
        };
        let msg =
            build_chat_user_message("hi", &[image, inline], std::path::Path::new("/workspace"));
        match msg.content.expect("content required") {
            MessageContent::Parts(parts) => {
                let image_count = parts
                    .iter()
                    .filter(|p| matches!(p, ContentPart::ImageUrl { .. }))
                    .count();
                assert_eq!(image_count, 1, "expected exactly one ImageUrl part");
                let has_caption = parts.iter().any(|p| {
                    matches!(p, ContentPart::Text { text } if text.contains("hi") && text.contains("hello"))
                });
                assert!(
                    has_caption,
                    "expected caption text with inline body present in Parts"
                );
            }
            other => panic!("expected MessageContent::Parts, got {other:?}"),
        }
        assert!(matches!(msg.role, Role::User));
    }

    #[test]
    fn build_chat_user_message_text_only_yields_message_content_text() {
        let inline = LocalAttachment::InlineText {
            header: "File: n.txt".to_string(),
            body: "hello".to_string(),
        };
        let msg = build_chat_user_message("hi", &[inline], std::path::Path::new("/workspace"));
        match msg.content.expect("content required") {
            MessageContent::Text(text) => {
                assert!(text.contains("hi"));
                assert!(text.contains("hello"));
            }
            other => panic!("expected MessageContent::Text (no Parts), got {other:?}"),
        }
    }

    #[test]
    fn build_chat_user_message_workspace_file_appends_note_without_body() {
        let workspace = LocalAttachment::WorkspaceFile {
            filename: "big.txt".to_string(),
            note: "exceeds inline threshold".to_string(),
        };
        let msg = build_chat_user_message(
            "here",
            &[workspace],
            std::path::Path::new("/session/workspace"),
        );
        match msg.content.expect("content required") {
            MessageContent::Text(text) => {
                assert!(
                    text.contains("/session/workspace") && text.contains("big.txt"),
                    "expected a workspace-path note referencing the file, got: {text}"
                );
            }
            other => panic!("expected MessageContent::Text, got {other:?}"),
        }
    }

    #[test]
    fn build_chat_user_message_empty_caption_with_image_still_valid() {
        let png = incompressible_png(64);
        let image = process_local_attachment(&png, "image/png", "a.png", PdfMode::TextExtract)
            .expect("image should classify");
        let msg = build_chat_user_message("", &[image], std::path::Path::new("/workspace"));
        assert!(matches!(msg.role, Role::User));
        match msg.content.expect("content required") {
            MessageContent::Parts(parts) => {
                assert!(
                    parts
                        .iter()
                        .any(|p| matches!(p, ContentPart::ImageUrl { .. }))
                );
            }
            other => panic!("expected MessageContent::Parts, got {other:?}"),
        }
    }

    #[test]
    fn build_chat_user_message_truncates_beyond_max_attachments_per_message() {
        let attachments: Vec<LocalAttachment> = (0..MAX_ATTACHMENTS_PER_MESSAGE + 3)
            .map(|i| LocalAttachment::InlineText {
                header: format!("File: f{i}.txt"),
                body: format!("body{i}"),
            })
            .collect();
        let msg = build_chat_user_message(
            "many files",
            &attachments,
            std::path::Path::new("/workspace"),
        );
        match msg.content.expect("content required") {
            MessageContent::Text(text) => {
                // Only the first MAX_ATTACHMENTS_PER_MESSAGE bodies should be
                // present; the surplus is noted, not silently dropped.
                assert!(text.contains("body0"));
                assert!(
                    !text.contains(&format!("body{}", MAX_ATTACHMENTS_PER_MESSAGE)),
                    "surplus attachment body must not be inlined"
                );
                assert!(
                    text.contains("3 additional attachment"),
                    "expected a surplus note, got: {text}"
                );
            }
            other => panic!("expected MessageContent::Text, got {other:?}"),
        }
    }
}
