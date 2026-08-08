//! LocalSet-safe async→sync bridge for the synchronous command handlers.
//!
//! ## Why this exists
//!
//! `commands::handlers::dispatch` and the `CommandContext` handle traits
//! (`SubagentListSnapshot`, `ProcessRegistrySnapshotHandle`,
//! `ToolsetSessionHandle`) are deliberately **synchronous** — they are called
//! from sync render/dispatch code on every surface. Their implementations,
//! however, guard state behind `tokio::sync::RwLock`, so each one needs an
//! async→sync bridge.
//!
//! The obvious bridge — `tokio::task::block_in_place` + `Handle::block_on` —
//! is **not** portable across our surfaces. `block_in_place` panics with
//! `"can call blocking only when running on the multi-threaded runtime"`
//! whenever the caller is on a current-thread runtime *or* inside a
//! `tokio::task::LocalSet`. The Dioxus fullstack server polls each websocket
//! server-fn handler inside a per-connection `LocalSet` on top of an otherwise
//! multi-threaded runtime, so the runtime flavor alone cannot be used to
//! predict whether `block_in_place` is legal — and there is no public tokio API
//! that reports it.
//!
//! Phase 26.7-07 already discovered this the hard way and solved it for
//! `RegistrationGuard::drop` by dispatching to a fresh OS thread, which carries
//! no tokio runtime affiliation and therefore escapes the `LocalSet`. Phase
//! 41.3 Plan 04 (`bc5f1c0c3`) wired Web's `CommandContext` from 2 handles to
//! 9-of-9, which routed `/agents` through the *other*, still-unconverted
//! bridges and reproduced the identical panic in UAT. This module generalizes
//! the proven 26.7-07 escape hatch so there is exactly one bridge to reason
//! about.
//!
//! ## Cost
//!
//! One short-lived OS thread per call, and the calling thread parks in `join()`
//! without announcing the block to the scheduler (which is what `block_in_place`
//! did). That is acceptable **only** because every caller is an interactive,
//! low-frequency operation — slash-command dispatch and toolset refreshes. Do
//! not reach for this on a per-frame or per-token path; restructure the caller
//! to be async instead.

use std::future::Future;

/// Run `fut` to completion from synchronous code, on any tokio context.
///
/// - Inside a runtime (multi-thread, current-thread, or `LocalSet`): the future
///   is driven by the current runtime via a scoped OS thread, so no `LocalSet`
///   or current-thread restriction applies. Scoped threads mean `fut` may
///   borrow — it does not need to be `'static`.
/// - Outside any runtime: a temporary current-thread runtime is built to drive
///   it. The previous `block_in_place` bridge panicked in this case
///   (`Handle::current()` outside a runtime), so this is strictly more
///   permissive.
///
/// A panic inside `fut` is resumed on the calling thread, preserving the
/// unwinding behavior callers had with `block_in_place`.
pub fn block_on_sync<F, R>(fut: F) -> R
where
    F: Future<Output = R> + Send,
    R: Send,
{
    match tokio::runtime::Handle::try_current() {
        Ok(handle) => std::thread::scope(|scope| {
            match scope.spawn(move || handle.block_on(fut)).join() {
                Ok(value) => value,
                // Propagate rather than swallow: callers (and their tests)
                // expect a panic in `fut` to surface at the call site.
                Err(payload) => std::panic::resume_unwind(payload),
            }
        }),
        Err(_) => tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("block_on_sync: failed to build fallback current-thread runtime")
            .block_on(fut),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use tokio::sync::RwLock;

    /// The regression case: a `LocalSet` over a multi-threaded runtime, which is
    /// exactly how Dioxus fullstack polls websocket server-fn handlers. The old
    /// `block_in_place` bridge panicked here.
    #[test]
    fn works_inside_a_localset() {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();
        let lock = Arc::new(RwLock::new(41_u32));
        let local = tokio::task::LocalSet::new();

        let got = local.block_on(&rt, async {
            // Sync context inside the LocalSet — the shape every handle trait
            // method has.
            block_on_sync(async { *lock.read().await + 1 })
        });

        assert_eq!(got, 42);
    }

    #[test]
    fn works_on_a_current_thread_runtime() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let lock = Arc::new(RwLock::new(7_u32));

        let got = rt.block_on(async { block_on_sync(async { *lock.read().await }) });

        assert_eq!(got, 7);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn works_on_a_multi_thread_runtime() {
        let lock = Arc::new(RwLock::new(3_u32));
        assert_eq!(block_on_sync(async { *lock.read().await }), 3);
    }

    #[test]
    fn works_with_no_runtime_at_all() {
        let lock = Arc::new(RwLock::new(5_u32));
        assert_eq!(block_on_sync(async { *lock.read().await }), 5);
    }

    /// Writes must land — the bridge is used for mutating operations
    /// (`kill`, `drain_and_kill`, toolset enable/disable), not just reads.
    #[test]
    fn write_locks_are_observable_after_the_bridge_returns() {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();
        let lock = Arc::new(RwLock::new(0_u32));
        let local = tokio::task::LocalSet::new();

        let observed = local.block_on(&rt, {
            let lock = lock.clone();
            async move {
                block_on_sync(async {
                    *lock.write().await = 99;
                });
                block_on_sync(async { *lock.read().await })
            }
        });

        assert_eq!(observed, 99);
    }

    #[test]
    #[should_panic(expected = "boom")]
    fn a_panic_inside_the_future_propagates_to_the_caller() {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();
        let local = tokio::task::LocalSet::new();

        local.block_on(&rt, async {
            block_on_sync(async { panic!("boom") });
        });
    }
}
