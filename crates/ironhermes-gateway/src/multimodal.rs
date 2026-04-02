use anyhow::{bail, Result};
use base64::Engine as _;
use tracing::{info, warn};

use crate::telegram::{TelegramAdapter, TgMessage};

/// Maximum file size accepted for download (20MB per D-08).
const MAX_FILE_SIZE: i64 = 20 * 1024 * 1024;

/// Result of processing a TgMessage's attachments.
pub struct ProcessedAttachments {
    /// Text to prepend to the user message (extracted document content).
    pub text_prefix: Option<String>,
    /// Base64-encoded image data URI for vision models.
    pub image_data_uri: Option<String>,
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
    if let Some(ref photos) = msg.photo {
        if let Some(largest) = photos.last() {
            // Check file size if known
            if let Some(size) = largest.file_size {
                if size > MAX_FILE_SIZE {
                    bail!("File too large (max 20MB). Please send a smaller image.");
                }
            }

            info!(file_id = %largest.file_id, "Downloading photo from Telegram");
            let tg_file = adapter.get_file(&largest.file_id).await?;
            let file_path = tg_file
                .file_path
                .as_deref()
                .ok_or_else(|| anyhow::anyhow!("Telegram returned no file_path for photo"))?;

            let bytes = adapter.download_file(file_path).await?;
            let encoded = base64::engine::general_purpose::STANDARD.encode(&bytes);
            let data_uri = format!("data:image/jpeg;base64,{}", encoded);

            return Ok(ProcessedAttachments {
                image_data_uri: Some(data_uri),
                text_prefix: None,
            });
        }
    }

    // --- Document ---
    if let Some(ref doc) = msg.document {
        if let Some(size) = doc.file_size {
            if size > MAX_FILE_SIZE {
                bail!("File too large (max 20MB). Please send a smaller file.");
            }
        }

        let mime = doc
            .mime_type
            .as_deref()
            .unwrap_or("application/octet-stream");
        let filename = doc
            .file_name
            .as_deref()
            .unwrap_or("document");

        info!(file_id = %doc.file_id, mime_type = %mime, filename = %filename, "Downloading document from Telegram");
        let tg_file = adapter.get_file(&doc.file_id).await?;
        let file_path = tg_file
            .file_path
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("Telegram returned no file_path for document"))?;

        let bytes = adapter.download_file(file_path).await?;

        let text = match mime {
            "application/pdf" => {
                match pdf_extract::extract_text_from_mem(&bytes) {
                    Ok(extracted) => extracted,
                    Err(e) => {
                        warn!(error = %e, "PDF text extraction failed, returning error");
                        bail!("Could not extract text from PDF: {}", e);
                    }
                }
            }
            "text/plain" | "text/markdown" | "text/csv" | "text/html" => {
                String::from_utf8_lossy(&bytes).into_owned()
            }
            other => {
                bail!("Unsupported document type: {}. Supported types: PDF, plain text, markdown, CSV.", other);
            }
        };

        let wrapped = format!("[Document: {}]\n{}", filename, text);
        return Ok(ProcessedAttachments {
            text_prefix: Some(wrapped),
            image_data_uri: None,
        });
    }

    // No attachment
    Ok(ProcessedAttachments {
        text_prefix: None,
        image_data_uri: None,
    })
}
