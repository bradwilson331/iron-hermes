use std::time::Duration;

/// Exponential backoff with jitter for polling error recovery (TG-07).
/// Base delay doubles on each failure: min(base * 2^n + jitter, cap).
/// Detects 409 conflicts as fatal after 5 consecutive failures.
pub struct BackoffState {
    base_ms: u64,
    cap_ms: u64,
    failures: u32,
    conflict_count: u32,
}

impl BackoffState {
    pub fn new(base_ms: u64, cap_ms: u64) -> Self {
        Self {
            base_ms,
            cap_ms,
            failures: 0,
            conflict_count: 0,
        }
    }

    /// Default: 1s base, 60s cap.
    pub fn default_polling() -> Self {
        Self::new(1_000, 60_000)
    }

    /// Compute next delay with jitter. Uses system time nanos as zero-dependency
    /// jitter source (acceptable for backoff — not cryptographic).
    pub fn next_delay(&self) -> Duration {
        let exp = self.base_ms.saturating_mul(1u64 << self.failures.min(10));
        let max_jitter = exp / 4 + 1;
        let jitter = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .subsec_nanos() as u64
            % max_jitter;
        Duration::from_millis((exp + jitter).min(self.cap_ms))
    }

    pub fn record_success(&mut self) {
        self.failures = 0;
        self.conflict_count = 0;
    }

    pub fn record_failure(&mut self) {
        self.failures = self.failures.saturating_add(1);
    }

    pub fn record_conflict(&mut self) {
        self.conflict_count = self.conflict_count.saturating_add(1);
        self.record_failure();
    }

    /// Returns true when 5+ consecutive 409 conflicts detected —
    /// indicates another bot instance is polling on the same token.
    pub fn is_fatal_conflict(&self) -> bool {
        self.conflict_count >= 5
    }

    pub fn failures(&self) -> u32 {
        self.failures
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_has_zero_failures() {
        let b = BackoffState::new(1_000, 60_000);
        assert_eq!(b.failures(), 0);
    }

    #[test]
    fn test_initial_delay_is_base() {
        let b = BackoffState::new(1_000, 60_000);
        let delay = b.next_delay();
        // base is 1000ms, jitter adds up to ~250ms (25% of base), so range is [1000, 1250]
        assert!(
            delay.as_millis() >= 1000 && delay.as_millis() <= 1250,
            "Expected ~1000ms, got {}ms",
            delay.as_millis()
        );
    }

    #[test]
    fn test_after_one_failure_delay_is_doubled() {
        let mut b = BackoffState::new(1_000, 60_000);
        b.record_failure();
        let delay = b.next_delay();
        // after 1 failure: 2^1 * 1000 = 2000ms + up to 500ms jitter
        assert!(
            delay.as_millis() >= 2000 && delay.as_millis() <= 2500,
            "Expected ~2000ms, got {}ms",
            delay.as_millis()
        );
    }

    #[test]
    fn test_after_five_failures_delay_is_32s() {
        let mut b = BackoffState::new(1_000, 60_000);
        for _ in 0..5 {
            b.record_failure();
        }
        let delay = b.next_delay();
        // 2^5 * 1000 = 32000ms + up to 8000ms jitter, capped at 60000
        assert!(
            delay.as_millis() >= 32000 && delay.as_millis() <= 60_000,
            "Expected ~32000ms, got {}ms",
            delay.as_millis()
        );
    }

    #[test]
    fn test_delay_never_exceeds_cap() {
        let mut b = BackoffState::new(1_000, 60_000);
        for _ in 0..20 {
            b.record_failure();
        }
        let delay = b.next_delay();
        assert!(
            delay.as_millis() <= 60_000,
            "Delay exceeded cap: {}ms",
            delay.as_millis()
        );
    }

    #[test]
    fn test_record_success_resets_failures() {
        let mut b = BackoffState::new(1_000, 60_000);
        b.record_failure();
        b.record_failure();
        b.record_failure();
        assert_eq!(b.failures(), 3);
        b.record_success();
        assert_eq!(b.failures(), 0);
        // After reset, delay should be back to ~base
        let delay = b.next_delay();
        assert!(
            delay.as_millis() <= 1250,
            "After reset delay should be ~base, got {}ms",
            delay.as_millis()
        );
    }

    #[test]
    fn test_is_fatal_conflict_false_at_four() {
        let mut b = BackoffState::new(1_000, 60_000);
        for _ in 0..4 {
            b.record_conflict();
        }
        assert!(!b.is_fatal_conflict(), "Should not be fatal at 4 conflicts");
    }

    #[test]
    fn test_is_fatal_conflict_true_at_five() {
        let mut b = BackoffState::new(1_000, 60_000);
        for _ in 0..5 {
            b.record_conflict();
        }
        assert!(b.is_fatal_conflict(), "Should be fatal at 5 conflicts");
    }

    #[test]
    fn test_jitter_adds_randomness() {
        // Run next_delay many times; the exact value should vary due to subsecond timing
        // At minimum, the jitter range is non-zero (max_jitter = base/4 + 1 = 251)
        // We can verify the delay is >= base (never subtracted) and <= base + 25%
        let b = BackoffState::new(1_000, 60_000);
        let delay = b.next_delay();
        // Delay must be at least base_ms
        assert!(
            delay.as_millis() >= 1000,
            "Jitter should not reduce below base: {}ms",
            delay.as_millis()
        );
        // The computed value includes some jitter (max_jitter = 251ms at base 1000ms)
        // We can't guarantee non-zero jitter in a single call (nanos % 251 could be 0),
        // but we verify it stays within bounds
        assert!(
            delay.as_millis() <= 1250,
            "Jitter should stay within 25% of base: {}ms",
            delay.as_millis()
        );
    }

    #[test]
    fn test_default_polling_constructor() {
        let b = BackoffState::default_polling();
        assert_eq!(b.failures(), 0);
        let delay = b.next_delay();
        assert!(delay.as_millis() >= 1000 && delay.as_millis() <= 1250);
    }
}
