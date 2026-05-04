//! Phase 25.2 D-15 / D-16: chunked summarization + synthesis pass for tier 3 (500K-2M chars).
//!
//! Splits content into `extract_cfg.summary_chunk_chars` segments (default 100K), summarizes each
//! chunk in parallel via `tokio::spawn` + `sem.acquire_owned()` (D-15 single-budget), joins outputs
//! with `---\n[Chunk N/M]\n` markers (D-16), and runs ONE final synthesis LLM call.
//!
//! D-17: the same `Arc<Semaphore>` is used for BOTH outer (per-URL) and inner (per-chunk) fan-out
//! — Plan 12 holds the outer permit when calling route_tiers, this module acquires inner permits
//! per chunk. Single budget by design.

use anyhow::{Result, anyhow};
use ironhermes_core::SummarizationClientHandle;
use ironhermes_core::config::ExtractConfig;
use std::sync::Arc;
use tokio::sync::Semaphore;
use tracing::{Instrument, debug, info_span};

use super::tiers::{
    SUMMARY_SYSTEM, SYNTHESIS_SYSTEM, TIER3_CHUNK_MAX_TOKENS, TIER3_SYNTHESIS_MAX_TOKENS,
};

/// D-15 / D-16: chunked tier-3 summarization. Splits content, summarizes chunks in parallel,
/// then runs one synthesis pass over the joined chunk summaries.
pub async fn summarize_chunked(
    content: &str,
    extract_cfg: &ExtractConfig,
    client: &Arc<dyn SummarizationClientHandle>,
    sem: &Arc<Semaphore>,
) -> Result<String> {
    let chunks = split_into_chunks(content, extract_cfg.summary_chunk_chars);
    let total = chunks.len();
    let chunked_span = info_span!("web_extract.summary.chunked", chunks = total);
    async move {
        debug!(
            "chunked: splitting {} chars into {} chunks of ~{} chars",
            content.chars().count(),
            total,
            extract_cfg.summary_chunk_chars
        );

        // Spawn one task per chunk; each acquires sem permit (D-15)
        let mut handles = Vec::with_capacity(total);
        for (i, chunk) in chunks.iter().enumerate() {
            let sem_c = sem.clone();
            let client_c = client.clone();
            let chunk_str = chunk.clone();
            let idx = i;
            let n = total;
            handles.push(tokio::spawn(async move {
                let _permit = sem_c
                    .acquire_owned()
                    .await
                    .map_err(|e| anyhow!("semaphore acquire failed: {}", e))?;
                let prompt = format!("Summarize chunk {}/{} of a document:", idx + 1, n);
                client_c
                    .summarize_call(prompt, chunk_str, TIER3_CHUNK_MAX_TOKENS)
                    .await
            }));
        }

        // Collect chunk summaries IN ORDER (not race-order)
        let mut summaries: Vec<String> = Vec::with_capacity(total);
        for (i, h) in handles.into_iter().enumerate() {
            let s = h
                .await
                .map_err(|e| anyhow!("chunk {} task panicked: {}", i + 1, e))??;
            summaries.push(format!("---\n[Chunk {}/{}]\n{}", i + 1, total, s));
        }

        // D-16 synthesis pass — single LLM call over all chunk summaries
        let _permit = sem
            .acquire()
            .await
            .map_err(|e| anyhow!("semaphore acquire failed: {}", e))?;
        let syn_span = info_span!("web_extract.summary.synthesis", chunk_count = total);
        let joined = summaries.join("\n\n");
        let _ = SUMMARY_SYSTEM; // explicit unused — synthesis uses its own system prompt
        client
            .summarize_call(
                SYNTHESIS_SYSTEM.to_string(),
                joined,
                TIER3_SYNTHESIS_MAX_TOKENS,
            )
            .instrument(syn_span)
            .await
    }
    .instrument(chunked_span)
    .await
}

/// Split content into chunks of approximately `chunk_chars` characters.
/// UTF-8 safe via `chars()` iteration — no string slicing on non-char-boundaries.
fn split_into_chunks(content: &str, chunk_chars: usize) -> Vec<String> {
    if chunk_chars == 0 || content.is_empty() {
        return vec![content.to_string()];
    }

    let mut chunks = Vec::new();
    let mut current = String::new();
    let mut current_count = 0;

    for c in content.chars() {
        current.push(c);
        current_count += 1;
        if current_count >= chunk_chars {
            chunks.push(std::mem::take(&mut current));
            current_count = 0;
        }
    }

    if !current.is_empty() {
        chunks.push(current);
    }

    if chunks.is_empty() {
        chunks.push(content.to_string());
    }

    chunks
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;

    struct CountingHandle {
        calls: std::sync::Mutex<u32>,
    }

    #[async_trait]
    impl SummarizationClientHandle for CountingHandle {
        async fn summarize_call(
            &self,
            _system: String,
            _user: String,
            _max_tokens: u32,
        ) -> anyhow::Result<String> {
            let mut c = self.calls.lock().unwrap();
            *c += 1;
            Ok(format!("CHUNK-SUMMARY-{}", c))
        }
    }

    #[test]
    fn split_into_chunks_basic() {
        let s = "a".repeat(250);
        let chunks = split_into_chunks(&s, 100);
        assert_eq!(chunks.len(), 3);
        assert_eq!(chunks[0].chars().count(), 100);
        assert_eq!(chunks[1].chars().count(), 100);
        assert_eq!(chunks[2].chars().count(), 50);
    }

    #[test]
    fn split_into_chunks_utf8_safe() {
        let s = "héllo".repeat(100); // é is multi-byte UTF-8
        let chunks = split_into_chunks(&s, 50);
        // Reassemble and verify lossless
        let joined: String = chunks.into_iter().collect();
        assert_eq!(joined, s);
    }

    #[test]
    fn split_into_chunks_empty_input() {
        let chunks = split_into_chunks("", 100);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0], "");
    }

    #[tokio::test]
    async fn summarize_chunked_call_count_matches_chunks_plus_synthesis() {
        let content = "a".repeat(600_000);
        let mut cfg = ExtractConfig::default();
        cfg.summary_chunk_chars = 100_000;

        let counting = Arc::new(CountingHandle {
            calls: std::sync::Mutex::new(0),
        });
        let client: Arc<dyn SummarizationClientHandle> = counting.clone();
        let sem = Arc::new(Semaphore::new(4));

        let _ = summarize_chunked(&content, &cfg, &client, &sem)
            .await
            .unwrap();

        let total_calls = *counting.calls.lock().unwrap();
        // 6 chunks + 1 synthesis = 7
        assert_eq!(
            total_calls, 7,
            "expected 6 chunk + 1 synthesis = 7 calls, got {}",
            total_calls
        );
    }

    #[tokio::test]
    async fn summarize_chunked_synthesis_uses_chunk_markers() {
        // Use a custom handle that captures the synthesis user prompt
        struct CapturingHandle {
            calls: std::sync::Mutex<Vec<(String, String)>>,
        }
        #[async_trait]
        impl SummarizationClientHandle for CapturingHandle {
            async fn summarize_call(
                &self,
                system: String,
                user: String,
                _: u32,
            ) -> anyhow::Result<String> {
                self.calls.lock().unwrap().push((system, user));
                Ok("SYNTH".into())
            }
        }
        let content = "x".repeat(250_000); // 3 chunks at 100K
        let mut cfg = ExtractConfig::default();
        cfg.summary_chunk_chars = 100_000;
        let cap = Arc::new(CapturingHandle {
            calls: std::sync::Mutex::new(Vec::new()),
        });
        let client: Arc<dyn SummarizationClientHandle> = cap.clone();
        let sem = Arc::new(Semaphore::new(4));

        let _ = summarize_chunked(&content, &cfg, &client, &sem)
            .await
            .unwrap();

        let calls = cap.calls.lock().unwrap();
        // Last call is the synthesis pass — should contain `---\n[Chunk 1/3]\n` style markers
        let (synth_system, synth_user) = calls.last().expect("at least one call");
        assert!(
            synth_system.contains("synthesizer")
                || synth_system.contains("Synthes")
                || synth_system == SYNTHESIS_SYSTEM,
            "synthesis call should use SYNTHESIS_SYSTEM (got {})",
            synth_system
        );
        assert!(
            synth_user.contains("[Chunk 1/3]"),
            "synthesis user prompt missing chunk marker: {}",
            &synth_user[..synth_user.len().min(200)]
        );
    }
}
