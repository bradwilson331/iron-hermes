//! Phase 39.1 (R39.1-03 / R39.1-04 / D-03 / D-04): Two-level semaphore concurrency gate.
//!
//! `ConcurrencyLayer` enforces two independent limits:
//!
//! 1. **Per-session cap** (`per_session`) — limits how many concurrent turns a single
//!    session can run simultaneously (default 3, configurable via
//!    `config.concurrency.session_turn_cap`).
//! 2. **Process-wide global ceiling** (`global`) — limits total concurrent turns across
//!    all sessions and all surfaces (default 32, configurable via
//!    `config.concurrency.global_turn_ceiling`).
//!
//! # Acquisition protocol
//!
//! Call `try_acquire` before spawning an agent turn. If it returns `Some((per, global))`,
//! store **both permits** alongside the turn's `JoinHandle` (or in the `TurnEntry`'s
//! owning struct). When the task completes or panics, the permits are dropped and the
//! semaphore capacity is released automatically (RAII — see Pitfall 2 / Pitfall 6 in
//! `async-agent-best-practices.md`).
//!
//! If `try_acquire` returns `None`, the new turn cannot start immediately — the caller
//! should fall back to the existing FIFO queue (`app_state.queue` / `SessionQueue`)
//! per D-03.
//!
//! # RAII contract
//!
//! Both `OwnedSemaphorePermit`s MUST be held for the lifetime of the turn task.
//! Dropping them early releases capacity prematurely; keeping them past task completion
//! holds capacity unnecessarily. The canonical pattern:
//!
//! ```ignore
//! if let Some((per_permit, global_permit)) = layer.try_acquire() {
//!     let entry = TurnEntry { …, cancel: token.clone() };
//!     registry.register(entry).await;
//!     tokio::spawn(async move {
//!         let _per = per_permit;      // held for lifetime of task
//!         let _global = global_permit; // held for lifetime of task
//!         run_agent_turn(…).await;
//!         registry.deregister(turn_id).await;
//!     });
//! } else {
//!     // fall back to FIFO queue
//! }
//! ```

use std::sync::Arc;

use tokio::sync::{OwnedSemaphorePermit, Semaphore};

/// Two-level semaphore gate for concurrent agent turns.
///
/// `Clone` yields a second handle to the **same** underlying semaphores — clone
/// freely when wiring into multiple surface components.
#[derive(Clone)]
pub struct ConcurrencyLayer {
    /// Per-session semaphore. Shared among all turns in a single session.
    pub per_session: Arc<Semaphore>,
    /// Process-wide semaphore. Shared across all sessions and all surfaces.
    pub global: Arc<Semaphore>,
}

impl ConcurrencyLayer {
    /// Create a new layer with the given per-session cap and global ceiling.
    ///
    /// Panics if either limit is 0 (a zero-permit semaphore would permanently block).
    pub fn new(session_cap: usize, global_ceiling: usize) -> Self {
        assert!(session_cap > 0, "session_cap must be > 0");
        assert!(global_ceiling > 0, "global_ceiling must be > 0");
        Self {
            per_session: Arc::new(Semaphore::new(session_cap)),
            global: Arc::new(Semaphore::new(global_ceiling)),
        }
    }

    /// Attempt a non-blocking acquire on both semaphores.
    ///
    /// Returns `Some((per_session_permit, global_permit))` if both semaphores have
    /// available capacity. Returns `None` if either semaphore is exhausted, so the
    /// caller can fall back to the existing FIFO queue (D-03).
    ///
    /// Acquires `per_session` first, then `global`. If `per_session` succeeds but
    /// `global` is exhausted, the `per_session` permit is released (dropped) before
    /// returning `None`.
    pub fn try_acquire(&self) -> Option<(OwnedSemaphorePermit, OwnedSemaphorePermit)> {
        let per = self.per_session.clone().try_acquire_owned().ok()?;
        let global = match self.global.clone().try_acquire_owned() {
            Ok(g) => g,
            Err(_) => {
                // per_session permit dropped here — releases capacity immediately.
                drop(per);
                return None;
            }
        };
        Some((per, global))
    }
}
