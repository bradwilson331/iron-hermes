//! Phase 54 Plan 05 (SC-5, D-19, ADR-0005 Finding 6): the load half of the
//! rate-limiter closure. Task 1 (`ironhermes-core`) proves the config load
//! path clamps out-of-range values; this file proves the resulting
//! `PerUserRateLimiter` actually REFUSES once its burst allowance is
//! exhausted, is scoped per-user (not global), and recovers after refill.
//! Neither half alone closes SC-5: a clamp that never reaches an enforcing
//! limiter is unproven, and a limiter that "works" only in unit isolation is
//! not the same as one constructed from the values a real config load
//! actually produces.
//!
//! This is a pure test addition. It does not modify, and must not modify,
//! `crates/ironhermes-gateway/src/rate_limiter.rs` or
//! `crates/ironhermes-gateway/src/handler.rs`.
//!
//! ## Why no tokio time control
//!
//! `PerUserRateLimiter`'s token bucket (`crates/ironhermes-gateway/src/rate_limiter.rs`)
//! is built on `std::time::Instant::now()`, not `tokio::time::Instant`. Tokio's
//! `test-util` time-pausing (`tokio::time::pause`/`advance`) only intercepts
//! its own `tokio::time` clock and has no effect on `std::time::Instant`, so
//! it cannot be used to simulate the passage of time here. The refill test
//! below therefore uses a small REAL `std::thread::sleep`, mirroring the
//! existing `test_tokens_refill_over_time` unit test already in
//! `rate_limiter.rs` (same technique, same crate, pre-dating this plan).
//!
//! ## Why the burst and rate are derived, not hardcoded
//!
//! Both the burst size and the sustained rate used below are computed from
//! `ironhermes_core::config::RateLimitConfig`'s own bounds constants (task 1
//! of this plan), not written as bare numeric literals in the
//! `PerUserRateLimiter::new(...)` call. A future change to those bounds
//! therefore keeps this test honest instead of silently testing a range the
//! bounds no longer permit.

use ironhermes_core::config::RateLimitConfig;
use ironhermes_gateway::rate_limiter::PerUserRateLimiter;

/// A small burst, derived from the lower bound rather than a bare literal,
/// so the test stays fast (small burst) while remaining a legitimate,
/// non-degenerate multi-attempt case (more than the single-attempt trivial
/// case `MIN_BURST_SIZE` alone would give).
fn test_burst_size() -> u32 {
    RateLimitConfig::MIN_BURST_SIZE + 2
}

/// A high sustained rate — the upper bound itself, which is a valid
/// (non-clamped) configuration value — so the refill test's real sleep can
/// stay short: at `MAX_MESSAGES_PER_MINUTE` tokens refill at roughly 16.7
/// per second, so a single token is available again well within 150ms.
fn test_messages_per_minute() -> u32 {
    RateLimitConfig::MAX_MESSAGES_PER_MINUTE
}

/// The first N = burst-size attempts for one user each succeed, individually
/// asserted, and the attempt immediately after is refused.
///
/// The loop below asserts EACH attempt as it happens — it fails at the exact
/// failing iteration with that iteration's number in the message — which is
/// what "assert each individually" means here. This is deliberately NOT an
/// aggregate check (e.g. counting successes and comparing the count to N, or
/// `.all(...)`): an aggregate would not distinguish "all N succeeded" from
/// "N-1 succeeded and one was refused early", which is exactly the
/// distinction SC-5 needs proven.
#[test]
fn rate_limiter_refuses_immediately_after_burst_exhausted() {
    let burst = test_burst_size();
    let limiter = PerUserRateLimiter::new(test_messages_per_minute(), burst);

    for attempt in 1..=burst {
        assert!(
            limiter.check_and_consume("user-a"),
            "attempt {attempt} of {burst} (the configured burst size) must succeed"
        );
    }
    assert!(
        !limiter.check_and_consume("user-a"),
        "the attempt immediately after the burst allowance ({burst}) is exhausted must be refused"
    );
}

/// A second, different user is unaffected by the first user's exhausted
/// burst. A test that only exercised one user could not distinguish a
/// per-user limiter from a global one — this is that distinguishing case.
#[test]
fn rate_limiter_is_per_user() {
    let burst = test_burst_size();
    let limiter = PerUserRateLimiter::new(test_messages_per_minute(), burst);

    for attempt in 1..=burst {
        assert!(
            limiter.check_and_consume("user-a"),
            "user-a attempt {attempt} of {burst} must succeed before exhaustion"
        );
    }
    assert!(
        !limiter.check_and_consume("user-a"),
        "user-a must be refused immediately after its own burst is exhausted"
    );

    assert!(
        limiter.check_and_consume("user-b"),
        "a second, different user must be unaffected by the first user's exhausted burst \
         — a per-user limiter must not treat this as a shared/global bucket"
    );
}

/// After enough real elapsed time for the token bucket to refill, a
/// previously refused user is admitted again — proving the refusal above is
/// throttling rather than a permanent lockout.
///
/// Real `std::thread::sleep`, not tokio time control — see the module doc
/// comment for why `PerUserRateLimiter`'s `std::time::Instant`-based bucket
/// cannot be advanced synthetically. At `MAX_MESSAGES_PER_MINUTE` (a fast
/// sustained rate), 150ms is comfortably more than one refill period, the
/// same margin the pre-existing `test_tokens_refill_over_time` unit test in
/// `rate_limiter.rs` uses at a slower configured rate.
#[test]
fn rate_limiter_admits_again_after_refill() {
    let burst = test_burst_size();
    let limiter = PerUserRateLimiter::new(test_messages_per_minute(), burst);

    for attempt in 1..=burst {
        assert!(
            limiter.check_and_consume("user-a"),
            "user-a attempt {attempt} of {burst} must succeed before exhaustion"
        );
    }
    assert!(
        !limiter.check_and_consume("user-a"),
        "user-a must be refused immediately after its own burst is exhausted"
    );

    std::thread::sleep(std::time::Duration::from_millis(150));

    assert!(
        limiter.check_and_consume("user-a"),
        "after enough real elapsed time for the bucket to refill, a previously refused user \
         must be admitted again — the refusal above is throttling, not a permanent lockout"
    );
}
