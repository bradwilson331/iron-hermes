//! Phase 25.2: web_extract tool — multi-format URL extraction (HTML/PDF/YouTube)
//! with tiered LLM summarization (D-01..D-28 in .planning/phases/25.2-web-extract-tools/25.2-CONTEXT.md).
//!
//! Wave 0 stub. Real impl lands in plans 25.2-01..25.2-14.

pub mod backends;
pub mod dispatch;
pub mod pdf;
pub mod sanitize;
pub mod summary;
pub mod youtube;

/// Phase 25.2 D-02 / D-07: per-URL extraction outcome.
/// Cross-crate plain-String envelope (no enum-rich error types) per Phase 22.4.2.2 / 26 D-18 convention.
/// `error: Some(msg)` indicates the URL failed to extract; `content` is empty in that case.
/// `error: None` indicates success; `content` holds the normalized Markdown (with inline title header per D-07 Option B).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ExtractionResult {
    pub url: String,
    pub title: String,
    pub content: String,
    pub error: Option<String>,
}

impl ExtractionResult {
    /// Constructor for the partial-success error envelope (D-02).
    pub fn error(url: impl Into<String>, error_code: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            title: String::new(),
            content: String::new(),
            error: Some(error_code.into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extraction_result_serializes_with_null_error() {
        let r = ExtractionResult {
            url: "https://example.com".into(),
            title: "T".into(),
            content: "C".into(),
            error: None,
        };
        let s = serde_json::to_string(&r).unwrap();
        assert!(s.contains(r#""error":null"#), "{}", s);
        assert!(s.contains(r#""content":"C""#), "{}", s);
    }

    #[test]
    fn extraction_result_error_constructor() {
        let r = ExtractionResult::error("https://example.com", "url_contains_secret");
        assert_eq!(r.error.as_deref(), Some("url_contains_secret"));
        assert!(r.content.is_empty());
        assert!(r.title.is_empty());
        assert_eq!(r.url, "https://example.com");
    }
}
