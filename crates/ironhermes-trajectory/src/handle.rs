//! Phase 25.3 Plan 6: trait-object handle for `TrajectoryWriter`.
//!
//! `ironhermes-core` is a strict leaf crate (no internal `path =` deps).
//! To allow `CommandContext` (in core) to hold a trajectory writer without
//! depending on the trajectory crate, we define `TrajectoryWriterHandle` as
//! a trait in core and provide this impl here. Mirrors the
//! `MemoryManagerHandle` (Phase 20) / `SummarizationClientHandle` (Phase 25.2)
//! cycle-break pattern.

use crate::TrajectoryWriter;
use ironhermes_core::commands::context::TrajectoryWriterHandle;
use std::sync::{Arc, Mutex};

/// Wraps `Arc<Mutex<TrajectoryWriter>>` and implements `TrajectoryWriterHandle`.
/// Construct with `TrajectoryWriterHandleImpl::new(Arc::new(Mutex::new(writer)))`,
/// then attach via `CommandContext::with_trajectory_writer(Arc::new(handle))`.
pub struct TrajectoryWriterHandleImpl {
    inner: Arc<Mutex<TrajectoryWriter>>,
}

impl TrajectoryWriterHandleImpl {
    pub fn new(inner: Arc<Mutex<TrajectoryWriter>>) -> Self {
        Self { inner }
    }
}

impl TrajectoryWriterHandle for TrajectoryWriterHandleImpl {
    fn append_json_line(&self, line: &str) -> anyhow::Result<()> {
        let mut guard = self
            .inner
            .lock()
            .map_err(|e| anyhow::anyhow!("trajectory writer mutex poisoned: {e}"))?;
        guard.append_raw_line(line)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TrajectoryWriter;
    use std::sync::{Arc, Mutex};
    use tempfile::tempdir;

    #[test]
    fn handle_appends_line_via_trait() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("trajectories.jsonl");
        let writer = TrajectoryWriter::open(&path).expect("open writer");
        let handle = TrajectoryWriterHandleImpl::new(Arc::new(Mutex::new(writer)));
        handle
            .append_json_line(r#"{"name":"mock","ts":"now"}"#)
            .expect("append");
        let body = std::fs::read_to_string(&path).expect("read back");
        assert!(body.contains(r#""name":"mock""#));
        assert!(body.ends_with("\n"));
    }
}
