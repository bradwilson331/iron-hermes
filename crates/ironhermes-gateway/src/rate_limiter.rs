//! Per-user inbound rate limiter using token bucket algorithm (D-20).
//! Excess messages are silently dropped (D-21).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// Retry wrapper for Telegram API calls that may hit 429 rate limits (D-19).
///
/// Relocated from `handler.rs` as `pub(crate)` per Phase 36.17.2 Plan 01 checker M2 —
/// `user_queue.rs` needs this helper and both modules can share it via `rate_limiter`.
pub(crate) async fn with_rate_limit_retry<F, Fut, T>(f: F) -> anyhow::Result<T>
where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = anyhow::Result<T>>,
{
    for attempt in 0..3u64 {
        match f().await {
            Ok(v) => return Ok(v),
            Err(e) => {
                let err_str = e.to_string();
                if err_str.contains("429") || err_str.contains("Too Many Requests") {
                    let wait = (attempt + 1) * 2; // 2s, 4s, 6s
                    tracing::warn!(
                        "Telegram rate limit hit, retrying in {}s (attempt {})",
                        wait,
                        attempt + 1
                    );
                    tokio::time::sleep(std::time::Duration::from_secs(wait)).await;
                    continue;
                }
                return Err(e);
            }
        }
    }
    anyhow::bail!("Bot is being rate limited, please wait")
}
use std::time::Instant;

#[derive(Clone)]
struct TokenBucketState {
    tokens: f64,
    last_refill: Instant,
}

/// Per-user inbound rate limiter using token bucket algorithm (D-20).
/// Excess messages are silently dropped (D-21).
///
/// Each unique `user_id` gets its own token bucket. Tokens refill at
/// `messages_per_minute / 60` tokens per second, capped at `burst_size`.
#[derive(Clone)]
pub struct PerUserRateLimiter {
    state: Arc<Mutex<HashMap<String, TokenBucketState>>>,
    messages_per_minute: f64,
    burst_size: f64,
}

impl PerUserRateLimiter {
    /// Create a new rate limiter with the given sustained rate and burst capacity.
    pub fn new(messages_per_minute: u32, burst_size: u32) -> Self {
        Self {
            state: Arc::new(Mutex::new(HashMap::new())),
            messages_per_minute: messages_per_minute as f64,
            burst_size: burst_size as f64,
        }
    }

    /// Check whether `user_id` has tokens available and consume one if so.
    ///
    /// Returns `true` if the message should be processed, `false` if rate-limited.
    pub fn check_and_consume(&self, user_id: &str) -> bool {
        let mut state = self.state.lock().unwrap();
        let now = Instant::now();

        let bucket = state
            .entry(user_id.to_string())
            .or_insert_with(|| TokenBucketState {
                tokens: self.burst_size,
                last_refill: now,
            });

        // Refill tokens based on elapsed time
        let elapsed = now.duration_since(bucket.last_refill).as_secs_f64();
        bucket.tokens =
            (bucket.tokens + elapsed * self.messages_per_minute / 60.0).min(self.burst_size);
        bucket.last_refill = now;

        // Try to consume a token
        if bucket.tokens >= 1.0 {
            bucket.tokens -= 1.0;
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_burst_allows_initial_messages() {
        let limiter = PerUserRateLimiter::new(10, 3);
        assert!(limiter.check_and_consume("user1"));
        assert!(limiter.check_and_consume("user1"));
        assert!(limiter.check_and_consume("user1"));
    }

    #[test]
    fn test_burst_exhausted_blocks() {
        let limiter = PerUserRateLimiter::new(10, 3);
        // Exhaust burst
        assert!(limiter.check_and_consume("user1"));
        assert!(limiter.check_and_consume("user1"));
        assert!(limiter.check_and_consume("user1"));
        // Should be blocked now
        assert!(!limiter.check_and_consume("user1"));
    }

    #[test]
    fn test_independent_user_buckets() {
        let limiter = PerUserRateLimiter::new(10, 3);
        // Exhaust user1's burst
        assert!(limiter.check_and_consume("user1"));
        assert!(limiter.check_and_consume("user1"));
        assert!(limiter.check_and_consume("user1"));
        assert!(!limiter.check_and_consume("user1"));
        // user2 should still have full burst
        assert!(limiter.check_and_consume("user2"));
        assert!(limiter.check_and_consume("user2"));
        assert!(limiter.check_and_consume("user2"));
    }

    #[test]
    fn test_tokens_refill_over_time() {
        let limiter = PerUserRateLimiter::new(600, 3); // 10 per second for fast test
        // Exhaust burst
        assert!(limiter.check_and_consume("user1"));
        assert!(limiter.check_and_consume("user1"));
        assert!(limiter.check_and_consume("user1"));
        assert!(!limiter.check_and_consume("user1"));
        // Wait for refill (100ms at 10/sec = 1 token)
        std::thread::sleep(std::time::Duration::from_millis(150));
        assert!(limiter.check_and_consume("user1"));
    }

    #[test]
    fn test_default_config_values() {
        let config = ironhermes_core::config::RateLimitConfig::default();
        assert_eq!(config.messages_per_minute, 10);
        assert_eq!(config.burst_size, 3);
    }
}
