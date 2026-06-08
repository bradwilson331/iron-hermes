//! KanbanStoreReaderImpl — concrete adapter implementing `KanbanStoreReader`
//! for the `/kanban` slash command handler.
//!
//! Phase 36.3.7.0 Plan 02 (BUG-36.3.7-02).
//!
//! Mirrors `CronJobReaderImpl` (crates/ironhermes-cli/src/tui_rata/commands.rs:534)
//! for the structural pattern: wraps `Arc<Mutex<KanbanStore>>`, implements the
//! core trait, and is created via `open_default()` at session start.
//!
//! The `KanbanStoreReader` trait lives in `ironhermes-core` (cycle-break).
//! This impl lives in `ironhermes-cli` (leaf crate) where `ironhermes-kanban`
//! is already a dependency.

use std::sync::{Arc, Mutex};

use ironhermes_core::commands::context::KanbanStoreReader;
use ironhermes_kanban::{KanbanStore, ListFilters};

use super::format::{format_task_detail, format_task_table};

/// Concrete implementation of `KanbanStoreReader` backed by `Arc<Mutex<KanbanStore>>`.
///
/// Created once at session start via `open_default()` (best-effort — on open failure
/// the CommandContext's `kanban_store` field stays None and `cmd_kanban` returns
/// the "not configured" message, matching `cmd_cron`'s line 1064 fallback shape).
pub struct KanbanStoreReaderImpl {
    store: Arc<Mutex<KanbanStore>>,
}

impl KanbanStoreReaderImpl {
    /// Construct from a pre-existing shared store handle.
    #[allow(dead_code)] // injection constructor for future test/DI wiring; production uses open_default()
    pub fn new(store: Arc<Mutex<KanbanStore>>) -> Self {
        Self { store }
    }

    /// Open the default KanbanStore path and wrap it in a shared handle.
    ///
    /// Returns `Err` when the DB cannot be opened (non-fatal — callers should
    /// log a warning and continue with `kanban_store: None`).
    pub fn open_default() -> ironhermes_kanban::Result<Self> {
        let store = KanbanStore::open_default()?;
        Ok(Self {
            store: Arc::new(Mutex::new(store)),
        })
    }
}

impl KanbanStoreReader for KanbanStoreReaderImpl {
    fn list_text(&self) -> String {
        let store = match self.store.lock() {
            Ok(s) => s,
            Err(_) => return "Failed to acquire kanban store lock.".to_string(),
        };
        match store.list_tasks(ListFilters::default()) {
            Ok(tasks) => format_task_table(&tasks),
            Err(e) => format!("Failed to read kanban tasks: {}", e),
        }
    }

    fn show_text(&self, id: &str) -> Option<String> {
        let store = match self.store.lock() {
            Ok(s) => s,
            Err(_) => return None,
        };
        let task = match store.get_task(id) {
            Ok(t) => t,
            Err(_) => return None,
        };
        // Fetch runs if available; pass empty slice for comments (no public get_comments API).
        let runs = store.get_runs(id).unwrap_or_default();
        let events = store.get_events(id).unwrap_or_default();
        Some(format_task_detail(&task, &runs, &events, &[]))
    }

    fn tip_text(&self) -> String {
        concat!(
            "Scratch workspaces live at ~/.ironhermes/kanban/workspaces/<task_id>/ and are wiped on kanban_complete.\n",
            "To preserve outputs, use `--workspace dir:/abs/path` or `--workspace worktree` on `ironhermes kanban create`.\n",
            "See `docs/kanban/worker-lanes.md` for the full workspace contract.\n",
        ).to_string()
    }

    fn deferred_subverb_message(&self, name: &str) -> String {
        format!(
            "/kanban {}: use 'ironhermes kanban {}' from the CLI for v1.",
            name, name
        )
    }
}
