//! Cross-platform async primitives for Phase 4's mock data layer.
//!
//! Per CONTEXT D-04: `sleep(ms)` is the only async primitive Phase 4 needs.
//! The cfg gate goes INSIDE the function body so the public signature is
//! identical on every platform. Callers import a single symbol.
//!
//! Web (wasm32): backed by `gloo_timers::future::TimeoutFuture`.
//! Native (desktop/mobile): backed by `tokio::time::sleep` with the
//! `time` feature only (per CONTEXT D-05).

/// Cross-platform async sleep.
///
/// On wasm32, suspends the current task for `ms` milliseconds via the
/// browser's `setTimeout`. On native targets, suspends via the tokio
/// time driver. Both branches are awaited; the public signature is
/// `pub async fn sleep(ms: u32)` on every platform.
///
/// Citations:
///   - gloo_timers::future::TimeoutFuture::new(u32):
///     docs.rs/gloo-timers/0.3.0/gloo_timers/future/struct.TimeoutFuture.html#method.new
///   - tokio::time::sleep(Duration) [features = "time"]:
///     docs.rs/tokio/1/tokio/time/fn.sleep.html
pub async fn sleep(ms: u32) {
    #[cfg(target_arch = "wasm32")]
    {
        gloo_timers::future::TimeoutFuture::new(ms).await;
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        tokio::time::sleep(std::time::Duration::from_millis(u64::from(ms))).await;
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
#[tokio::test]
async fn desktop_sleep_does_not_panic() {
    sleep(10).await;
}
