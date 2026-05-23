//! Phase 32.3 Plan 03 (D-08 / D-10): Shrike service — the operator-facing
//! termination/diagnostic surface for delegated subagents.
//!
//! Four library-level operations callable from any surface (TUI / web /
//! gateway):
//!
//! - [`ShrikeService::kill`] — hard-kill: cancels `CancellationToken` AND
//!   aborts the spawned `JoinHandle` so a wedged child cannot survive past
//!   the operator's intent. RegistrationGuard's `Drop` then deregisters
//!   the entry naturally (closes the 6.7-hour ghost class structurally).
//! - [`ShrikeService::interrupt`] — soft cancel: cancels the
//!   `CancellationToken` only. The child finalizes its current iteration
//!   before the cancel-check causes an early return.
//! - [`ShrikeService::prune`] — sweep registry entries whose
//!   `info.activity_last.elapsed() > stale_warn_seconds` (the threshold
//!   resolved per-call at register time in Plan 02). Returns the pruned
//!   ids.
//! - [`ShrikeService::status`] — diagnostic snapshot: returns a
//!   [`SubagentStatusInfo`](ironhermes_core::commands::context::SubagentStatusInfo)
//!   with id/parent_id/task/role/depth/uptime/last_activity/turns/transcript/status.
//!
//! The service holds two `Arc` fields:
//!
//! - `registry: Arc<RwLock<SubagentRegistry>>` — the canonical
//!   in-memory registry from Plan 01 / Plan 02. ShrikeService bridges
//!   sync-trait callers to the async lock via `block_in_place` +
//!   `block_on`, identical to `SubagentRegistryHandle::kill` (subagent_registry.rs:300–304).
//! - `active_handles: Arc<Mutex<HashMap<SubagentId, JoinHandle<()>>>>` —
//!   the JoinHandle map populated by `DelegateTaskTool::execute_batch`
//!   after `tokio::spawn`. `kill` aborts entries here; `interrupt` does
//!   NOT touch this map (the child gets to finalize naturally).
//!
//! **Why a dedicated module (D-10 Claude's Discretion):** RESEARCH §3 and
//! §5 both recommend centralizing the four operations so the contract is
//! testable in one place. Per-surface adapters (Plan 04) reach for these
//! same methods via the `SubagentListSnapshot` trait — no surface owns a
//! copy of the kill/interrupt/prune/status logic.
//!
//! **Constraint:** All four methods use the `block_in_place` + `block_on`
//! bridge to bridge the tokio async lock into the sync `SubagentListSnapshot`
//! trait surface. This is only safe on the tokio multi-thread runtime —
//! tests must use `#[tokio::test(flavor = "multi_thread", worker_threads = 2)]`.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tokio::sync::RwLock;
use tokio::task::JoinHandle;

use crate::subagent_registry::{SubagentId, SubagentRegistry};
use ironhermes_core::commands::context::SubagentStatusInfo;

/// Result of a successful [`ShrikeService::kill`] — diagnostic info captured
/// at the moment of kill so the operator output reads
/// `Killed sub_xxxxxxxx (ran NNNs, NN turns)` even though the registry
/// entry is gone immediately after.
#[derive(Debug, Clone)]
pub struct KillResult {
    pub id: String,
    pub uptime_secs: u64,
    /// Turns used by this subagent. Plan 03 stores 0 here because the
    /// per-child turn counter is not yet plumbed onto `SubagentInfo`
    /// (Phase 32.1 ActivityTracker tracks activity but not iteration
    /// count exposed back to the registry). Plan 04 may surface this
    /// once the tracker exposes a count accessor. Conservatively
    /// reported as 0 today rather than a misleading "iterations".
    pub turns_used: u32,
}

impl std::fmt::Display for KillResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Killed {} (ran {}s, {} turns)",
            self.id, self.uptime_secs, self.turns_used
        )
    }
}

/// Phase 32.3 Plan 03 (D-08 / D-10): operator-facing termination surface.
///
/// Construction: pass the same `Arc<RwLock<SubagentRegistry>>` that wraps
/// the session registry. `SubagentRegistryHandle::new` builds one
/// internally so the trait override on the handle can forward through
/// here.
///
/// The service is cheap to clone (two `Arc`s); typically one instance
/// per session.
#[derive(Clone)]
pub struct ShrikeService {
    registry: Arc<RwLock<SubagentRegistry>>,
    active_handles: Arc<Mutex<HashMap<SubagentId, JoinHandle<()>>>>,
}

impl ShrikeService {
    /// Build a new ShrikeService around an existing
    /// `Arc<RwLock<SubagentRegistry>>`. The active-handles map starts empty;
    /// `DelegateTaskTool::execute_batch` populates it after each
    /// `tokio::spawn` via [`Self::handle_map`].
    pub fn new(registry: Arc<RwLock<SubagentRegistry>>) -> Self {
        Self {
            registry,
            active_handles: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Accessor for the shared `JoinHandle` map. `DelegateTaskTool::execute_batch`
    /// inserts the handle returned by `tokio::spawn` keyed by `SubagentId`,
    /// and the spawned future itself removes its own entry on natural
    /// completion (the RegistrationGuard already handles the registry
    /// deregistration; this map cleanup is a separate hook because
    /// JoinHandle is not Clone).
    pub fn handle_map(&self) -> Arc<Mutex<HashMap<SubagentId, JoinHandle<()>>>> {
        self.active_handles.clone()
    }

    /// D-08 hard-kill: cancel `CancellationToken` AND abort the spawned
    /// `JoinHandle`. The RegistrationGuard's Drop deregisters the registry
    /// entry once the dropped future settles (closes the 6.7-hour ghost).
    ///
    /// Returns `Some(KillResult)` with uptime captured before the cancel,
    /// or `None` when the id is not present.
    pub fn kill(&self, id: &str) -> Option<KillResult> {
        let kill_result: Option<KillResult> = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async {
                let mut guard = self.registry.write().await;
                let info = guard.get(id)?.clone();
                let uptime_secs = info.started_at.elapsed().as_secs();
                // W3: cancel the token. The bare registry kill removes the
                // entry too — but we want the Drop of the spawned-future's
                // RegistrationGuard to do that so the dedup HashSet sweep
                // (`stale_warned`) happens via the canonical path. So we
                // only cancel the token here, not remove the entry.
                info.cancel.cancel();
                Some(KillResult {
                    id: info.id.clone(),
                    uptime_secs,
                    turns_used: 0,
                })
            })
        });

        // W3 / D-08 abort: if the JoinHandle is in our map, abort it.
        // This is what makes kill *more* than interrupt — a wedged future
        // that ignores its CancellationToken is force-aborted via tokio's
        // task-level abort.
        if let Some(ref kr) = kill_result {
            if let Ok(mut handles) = self.active_handles.lock() {
                if let Some(handle) = handles.remove(&kr.id) {
                    handle.abort();
                }
            }
            tracing::info!(
                target: "ironhermes_agent::shrike",
                subagent_id = %kr.id,
                uptime_secs = kr.uptime_secs,
                "shrike: killed subagent (token cancel + JoinHandle::abort)"
            );
        } else {
            tracing::warn!(
                target: "ironhermes_agent::shrike",
                subagent_id = %id,
                "shrike: kill no-op — id not present in registry"
            );
        }

        kill_result
    }

    /// D-08 soft interrupt: cancel `CancellationToken` only. Does NOT
    /// touch the JoinHandle map — the child finalizes the current
    /// iteration cooperatively before its cancel check exits.
    ///
    /// Returns `true` when the id was present, `false` otherwise.
    pub fn interrupt(&self, id: &str) -> bool {
        let cancelled: bool = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async {
                let guard = self.registry.read().await;
                match guard.get(id) {
                    Some(info) => {
                        info.cancel.cancel();
                        true
                    }
                    None => false,
                }
            })
        });

        if cancelled {
            tracing::info!(
                target: "ironhermes_agent::shrike",
                subagent_id = %id,
                "shrike: interrupted subagent (token cancel only, child finalizes)"
            );
        } else {
            tracing::warn!(
                target: "ironhermes_agent::shrike",
                subagent_id = %id,
                "shrike: interrupt no-op — id not present in registry"
            );
        }
        cancelled
    }

    /// D-08 stale sweep: collect every registry entry whose
    /// `info.activity_last.elapsed() > stale_secs`. For each match, cancel
    /// the token + abort the handle if present. Returns the pruned ids.
    ///
    /// Two-pass pattern (RESEARCH T-32.3-10 mitigation): read-list under a
    /// read lock first, drop the read lock, then per-id cancel under a
    /// fresh read lock. Avoids holding the write lock across the iteration
    /// (the actual entry removal happens via the spawned future's
    /// RegistrationGuard Drop, same as kill).
    pub fn prune(&self, stale_secs: u64) -> Vec<SubagentId> {
        let stale_ids: Vec<SubagentId> = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async {
                let guard = self.registry.read().await;
                guard
                    .list()
                    .into_iter()
                    .filter_map(|info| {
                        let al = info.activity_last.as_ref()?;
                        let elapsed = al.lock().ok()?.elapsed().as_secs();
                        if elapsed > stale_secs {
                            Some(info.id)
                        } else {
                            None
                        }
                    })
                    .collect()
            })
        });

        // Cancel each stale entry's token (same as interrupt semantics for
        // the soft-cancel path), then abort the JoinHandle if present.
        // The RegistrationGuard Drop on the future will deregister the
        // entry naturally.
        for id in &stale_ids {
            tokio::task::block_in_place(|| {
                tokio::runtime::Handle::current().block_on(async {
                    let guard = self.registry.read().await;
                    if let Some(info) = guard.get(id) {
                        info.cancel.cancel();
                    }
                })
            });
            if let Ok(mut handles) = self.active_handles.lock() {
                if let Some(handle) = handles.remove(id) {
                    handle.abort();
                }
            }
        }

        tracing::info!(
            target: "ironhermes_agent::shrike",
            count = stale_ids.len(),
            stale_secs,
            "shrike: pruned stale subagents"
        );
        stale_ids
    }

    /// D-08 diagnostic: snapshot the registry entry for `id` as a
    /// cross-crate-safe [`SubagentStatusInfo`]. Returns `None` when the id
    /// is not present.
    pub fn status(&self, id: &str) -> Option<SubagentStatusInfo> {
        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async {
                let guard = self.registry.read().await;
                let info = guard.get(id)?;
                let uptime_secs = info.started_at.elapsed().as_secs();
                let last_activity_secs = info
                    .activity_last
                    .as_ref()
                    .and_then(|al| al.lock().ok().map(|g| g.elapsed().as_secs()));
                let derived_status = if info.cancel.is_cancelled() {
                    "killed".to_string()
                } else if let Some(idle) = last_activity_secs {
                    if idle > info.stale_warn_seconds {
                        "stale".to_string()
                    } else {
                        "running".to_string()
                    }
                } else {
                    "running".to_string()
                };
                Some(SubagentStatusInfo {
                    id: info.id.clone(),
                    parent_id: info.parent_id.clone(),
                    task_summary: info.task_summary.clone(),
                    // Plan 03 does NOT plumb role/depth onto SubagentInfo —
                    // both are runner-local in Phase 32.2 D-01. Surface as
                    // None for now; Plan 04 may thread them if needed.
                    role: None,
                    depth: None,
                    uptime_secs,
                    last_activity_secs,
                    // Phase 32.1 ActivityTracker tracks activity bumps but
                    // does not yet expose an iteration count back to the
                    // registry. Conservatively None today.
                    turns_used: None,
                    transcript_path: info.transcript_path.display().to_string(),
                    status: derived_status,
                })
            })
        })
    }
}
