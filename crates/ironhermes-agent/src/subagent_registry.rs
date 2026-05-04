//! In-memory session-scoped subagent registry (D-03, D-04, D-09).
//!
//! Populated by the existing `SubagentProgressCallback` in
//! crates/ironhermes-cli/src/main.rs — wired in Wave 2 Plan 07.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Instant;
use tokio_util::sync::CancellationToken;

pub type SubagentId = String;

#[derive(Clone)]
pub struct SubagentInfo {
    pub id: SubagentId,
    pub task_summary: String,
    pub parent_id: Option<SubagentId>,
    pub started_at: Instant,
    pub cancel: CancellationToken,
    pub transcript_path: PathBuf,
}

#[derive(Default)]
pub struct SubagentRegistry {
    active: HashMap<SubagentId, SubagentInfo>,
}

impl SubagentRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, info: SubagentInfo) {
        self.active.insert(info.id.clone(), info);
    }

    pub fn unregister(&mut self, id: &str) -> Option<SubagentInfo> {
        self.active.remove(id)
    }

    pub fn active_count(&self) -> usize {
        self.active.len()
    }

    pub fn list(&self) -> Vec<SubagentInfo> {
        self.active.values().cloned().collect()
    }

    /// D-03 `/agents kill <id>`. Returns true if the id was present.
    /// Cancels the stored `CancellationToken` and removes the entry.
    pub fn kill(&mut self, id: &str) -> bool {
        match self.active.remove(id) {
            Some(info) => {
                info.cancel.cancel();
                true
            }
            None => false,
        }
    }

    pub fn transcript_path(&self, id: &str) -> Option<PathBuf> {
        self.active.get(id).map(|i| i.transcript_path.clone())
    }

    pub fn get(&self, id: &str) -> Option<&SubagentInfo> {
        self.active.get(id)
    }
}

/// Plan 21.7-07 (D-03 / D-09): newtype wrapper around
/// `Arc<RwLock<SubagentRegistry>>` implementing `SubagentListSnapshot`.
/// Newtype required by Rust's orphan rule (foreign trait on foreign type).
///
/// All methods are SYNC by the trait definition, but the underlying lock is
/// `tokio::sync::RwLock`. We use `tokio::task::block_in_place` +
/// `Handle::current().block_on` to bridge — the same pattern used by
/// `ironhermes-core/src/commands/handlers.rs` for `/models refresh`. Safe on
/// the tokio multi-thread runtime; locks uncontended in practice
/// (single-session registry).
#[derive(Clone)]
pub struct SubagentRegistryHandle(pub std::sync::Arc<tokio::sync::RwLock<SubagentRegistry>>);

impl SubagentRegistryHandle {
    pub fn new(reg: std::sync::Arc<tokio::sync::RwLock<SubagentRegistry>>) -> Self {
        Self(reg)
    }
}

impl ironhermes_core::commands::context::SubagentListSnapshot for SubagentRegistryHandle {
    fn active_count(&self) -> usize {
        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async { self.0.read().await.active_count() })
        })
    }

    fn list_summary(&self) -> Vec<(String, String, std::time::Duration)> {
        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async {
                let guard = self.0.read().await;
                guard
                    .list()
                    .into_iter()
                    .map(|info| {
                        let uptime = info.started_at.elapsed();
                        (info.id, info.task_summary, uptime)
                    })
                    .collect()
            })
        })
    }

    fn kill(&self, id: &str) -> bool {
        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async { self.0.write().await.kill(id) })
        })
    }

    fn transcript_path(&self, id: &str) -> Option<PathBuf> {
        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current()
                .block_on(async { self.0.read().await.transcript_path(id) })
        })
    }
}
