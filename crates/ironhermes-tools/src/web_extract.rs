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
