//! Phase 25.2 D-11 / D-12 / D-13: char-tier router for content summarization.
//!
//! Tier 1 (< 5K):       return raw Markdown — no LLM call.
//! Tier 2 (5K-500K):    single-pass aux LLM summary.
//! Tier 3 (500K-2M):    chunked parallel summaries + single synthesis pass (delegates to chunked.rs).
//! Tier 4 (> 2M):       refuse with Err("content_too_large") regardless of use_llm_processing.
//!
//! When `use_llm_processing == false` (D-12): tier 1 raw still works, tiers 2/3 fall back to
//! `truncate_content(content, web_cfg.max_content_chars)`, tier 4 still refuses.

use anyhow::{Result, anyhow};
use ironhermes_core::SummarizationClientHandle;
use ironhermes_core::config::{ExtractConfig, WebConfig};
use std::sync::Arc;
use tokio::sync::Semaphore;
use tracing::{Instrument, debug, info_span};

use super::chunked;
use crate::web_local::truncate_content;

/// System prompt for tier-2 single-pass summary and tier-3 per-chunk summaries.
pub const SUMMARY_SYSTEM: &str = "You are a content summarizer. Summarize the user's text \
    into a clear, concise Markdown overview that preserves key facts, numbers, named entities, \
    and structural headings. Do not invent information. Output Markdown only.";

/// System prompt for the tier-3 final synthesis pass over chunk summaries (D-16).
pub const SYNTHESIS_SYSTEM: &str = "You are a synthesizer. The user provides N chunk summaries \
    of a single document, separated by `---\\n[Chunk N/M]\\n` markers. Combine them into one \
    coherent overview. Preserve key facts, numbers, and named entities. Output Markdown only.";

/// Output token cap for tier 2 (single-pass summary).
pub const TIER2_MAX_TOKENS: u32 = 20_000;
/// Output token cap for tier 3 per-chunk summaries.
pub const TIER3_CHUNK_MAX_TOKENS: u32 = 5_000;
/// Output token cap for tier 3 final synthesis pass.
pub const TIER3_SYNTHESIS_MAX_TOKENS: u32 = 20_000;

/// D-11 / D-12: route the (already-extracted-and-sanitized) content to the appropriate tier.
/// Returns the user-facing content string (raw, summary, or synthesis output).
/// `Err("content_too_large")` for tier 4 (D-11 refuse).
pub async fn route_tiers(
    content: &str,
    use_llm_processing: bool,
    extract_cfg: &ExtractConfig,
    web_cfg: &WebConfig,
    client: &Arc<dyn SummarizationClientHandle>,
    sem: &Arc<Semaphore>,
) -> Result<String> {
    let n = content.chars().count();
    let tier_span = info_span!("web_extract.summary.tier", chars = n);
    async move {
        debug!(
            "tier router: content={} chars, use_llm={}",
            n, use_llm_processing
        );

        // Tier 4: refuse, regardless of use_llm_processing (D-11 + D-12)
        if n > extract_cfg.refuse_threshold_chars {
            return Err(anyhow!("content_too_large"));
        }

        // Tier 1: raw (always, no LLM)
        if n < extract_cfg.summary_tier2_threshold_chars {
            return Ok(content.to_string());
        }

        // D-12: use_llm_processing=false short-circuits tiers 2/3 — fall back to truncation
        if !use_llm_processing {
            return Ok(truncate_content(content, web_cfg.max_content_chars));
        }

        // Tier 2: single-pass summary (5K-500K)
        if n <= extract_cfg.summary_tier3_threshold_chars {
            let _permit = sem
                .acquire()
                .await
                .map_err(|e| anyhow!("semaphore acquire failed: {}", e))?;
            let aux_span = info_span!(
                "web_extract.summary.aux_call",
                tier = 2,
                max_tokens = TIER2_MAX_TOKENS
            );
            return client
                .summarize_call(
                    SUMMARY_SYSTEM.to_string(),
                    content.to_string(),
                    TIER2_MAX_TOKENS,
                )
                .instrument(aux_span)
                .await;
        }

        // Tier 3: chunked + synthesis (500K-2M) — delegate to chunked module
        chunked::summarize_chunked(content, extract_cfg, client, sem).await
    }
    .instrument(tier_span)
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;

    // Minimal test handle that records calls without making real LLM requests.
    struct TestHandle {
        calls: std::sync::Mutex<Vec<(String, u32)>>,
        response: String,
    }

    #[async_trait]
    impl SummarizationClientHandle for TestHandle {
        async fn summarize_call(
            &self,
            _system: String,
            user: String,
            max_tokens: u32,
        ) -> anyhow::Result<String> {
            self.calls
                .lock()
                .unwrap()
                .push((user.chars().take(20).collect(), max_tokens));
            Ok(self.response.clone())
        }
    }

    fn test_setup() -> (
        Arc<dyn SummarizationClientHandle>,
        Arc<Semaphore>,
        ExtractConfig,
        WebConfig,
    ) {
        let handle = Arc::new(TestHandle {
            calls: std::sync::Mutex::new(Vec::new()),
            response: "MOCK SUMMARY".to_string(),
        }) as Arc<dyn SummarizationClientHandle>;
        let sem = Arc::new(Semaphore::new(4));
        (handle, sem, ExtractConfig::default(), WebConfig::default())
    }

    #[tokio::test]
    async fn tier1_returns_raw_below_5k() {
        let (h, s, e, w) = test_setup();
        let content = "a".repeat(4_000);
        let out = route_tiers(&content, true, &e, &w, &h, &s).await.unwrap();
        assert_eq!(out.chars().count(), 4_000, "tier 1 returns raw");
    }

    #[tokio::test]
    async fn tier4_refuses_above_2m() {
        let (h, s, e, w) = test_setup();
        let content = "a".repeat(2_000_001);
        let err = route_tiers(&content, true, &e, &w, &h, &s)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("content_too_large"), "{err}");
    }

    #[tokio::test]
    async fn tier4_refuses_even_with_use_llm_false() {
        let (h, s, e, w) = test_setup();
        let content = "a".repeat(2_000_001);
        let err = route_tiers(&content, false, &e, &w, &h, &s)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("content_too_large"), "{err}");
    }

    #[tokio::test]
    async fn use_llm_false_short_circuits_tier2() {
        let (h, s, e, mut w) = test_setup();
        w.max_content_chars = 50;
        let content = "x".repeat(50_000); // tier 2 range
        let out = route_tiers(&content, false, &e, &w, &h, &s).await.unwrap();
        assert!(
            out.chars().count() <= 200,
            "use_llm=false → truncate to ~max_content_chars (got {})",
            out.chars().count()
        );
        assert!(
            out.contains("[Content truncated"),
            "expected truncation marker"
        );
    }
}
