//! Per-route fixed-window rate limiter (D-15).
//!
//! Fixed window, not a token bucket: D-15 specifies requests per minute, the
//! burst behaviour at a window boundary is acceptable for webhook traffic,
//! and a simpler structure has fewer ways to be wrong. Reuses the exact
//! [`crate::webhook::idempotency::Clock`] abstraction
//! [`crate::webhook::idempotency::IdempotencyCache`] (Task 2) introduced,
//! rather than adding a second notion of time to the same handler.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::webhook::idempotency::{Clock, SystemClock};

struct WindowState {
    window_start: Instant,
    count: u32,
}

/// One route's fixed-window rate limiter. Scoped to a single route by
/// construction (mirrors [`crate::webhook::idempotency::IdempotencyCache`]'s
/// per-route design) — built from that route's own
/// `rails.rate_limit_per_minute` at `WebhookAdapter::new` time and held
/// immutably thereafter (D-17): nothing reconfigures the limit while the
/// listener runs.
pub struct FixedWindowLimiter {
    limit: u32,
    window: Duration,
    clock: Arc<dyn Clock>,
    state: Mutex<WindowState>,
}

impl FixedWindowLimiter {
    /// Production constructor — the real system clock, a 60-second window.
    pub fn new(limit_per_minute: u32) -> Self {
        Self::with_clock(limit_per_minute, Arc::new(SystemClock))
    }

    /// Test/advanced constructor accepting an injectable clock.
    pub fn with_clock(limit_per_minute: u32, clock: Arc<dyn Clock>) -> Self {
        let now = clock.now();
        Self {
            limit: limit_per_minute,
            window: Duration::from_secs(60),
            clock,
            state: Mutex::new(WindowState {
                window_start: now,
                count: 0,
            }),
        }
    }

    /// Admit one request against this route's current window. Rolls the
    /// window forward (resetting the count to zero) when the injected clock
    /// has passed the window's end; admits while the count is under the
    /// configured limit, refuses otherwise.
    pub fn admit(&self) -> bool {
        let now = self.clock.now();
        let mut state = self.state.lock().unwrap();
        if now.duration_since(state.window_start) >= self.window {
            state.window_start = now;
            state.count = 0;
        }
        if state.count >= self.limit {
            return false;
        }
        state.count += 1;
        true
    }

    /// The configured limit — surfaced in the 429 response body so an
    /// operator debugging a throttled sender sees the number.
    pub fn limit(&self) -> u32 {
        self.limit
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::webhook::idempotency::FakeClock;

    #[test]
    fn admits_up_to_the_limit_then_refuses() {
        let limiter = FixedWindowLimiter::new(2);
        assert!(limiter.admit());
        assert!(limiter.admit());
        assert!(!limiter.admit());
    }

    #[test]
    fn window_rolling_resets_the_count() {
        let clock = Arc::new(FakeClock::new());
        let limiter = FixedWindowLimiter::with_clock(1, clock.clone());
        assert!(limiter.admit());
        assert!(!limiter.admit());

        clock.advance(Duration::from_secs(61));
        assert!(limiter.admit(), "the window rolled, so a fresh admit must succeed");
    }

    #[test]
    fn independent_limiters_do_not_share_state() {
        let a = FixedWindowLimiter::new(1);
        let b = FixedWindowLimiter::new(1);
        assert!(a.admit());
        assert!(!a.admit());
        // b's own budget is untouched by a's exhaustion.
        assert!(b.admit());
    }
}
