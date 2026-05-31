//! IronHermes Kanban — durable, profile-aware work-queue kernel.
//!
//! This crate owns the `~/.ironhermes/kanban.db` SQLite board and the
//! atomic-claim CAS helpers, dispatcher primitives, and worker-side DB API
//! used by the kanban subsystem (Phase 36.3.7).
//!
//! Plan 01 scaffolds the public type/path/event/config surface that
//! downstream plans consume:
//!
//! - Plan 02 adds `store.rs` / `cas.rs` / `schema.rs` (SQLite owner).
//! - Plan 03 adds `dispatcher.rs` / `worker_spawn.rs` (tokio task).
//! - Plan 04 adds `tools/` (6 LLM Tool impls).
//! - Plan 05 replaces the [`KANBAN_GUIDANCE`] placeholder with the
//!   canonical block.
//! - Plan 06 wires `running_agent::is_bypass()` to allow `kanban` mid-turn.
//!
//! All cross-crate types use plain `String` at the boundary per
//! CONTEXT.md D-17 — [`KanbanStatus`] / [`KanbanEventKind`] live in this
//! crate for internal type safety but [`Task::status`] /
//! [`KanbanEvent::kind`] are plain `String` so consumer crates don't take
//! a dep on us for the enum.

pub mod board;
pub mod cas;
pub mod config;
pub mod dispatcher;
pub mod error;
pub mod events;
pub mod kanban_guidance;
pub mod notifier;
pub mod paths;
pub mod pid;
pub mod schema;
pub mod skills_bundle;
pub mod store;
// Phase 36.3.7.5 BUG-36.3.7.5-06: production KanbanStoreWriter trait impl,
// consumed by the gateway's handle_slash_command attach site (handler.rs:430).
// Lives in ironhermes-kanban (not ironhermes-cli) because ironhermes-cli
// depends on ironhermes-gateway — reverse dep would be circular.
pub mod store_writer_impl;
pub mod mention;
pub mod tools;
pub mod types;
pub mod worker_spawn;

pub use board::{BoardContext, BoardSource, resolve_board_context};
pub use cas::{
    DEFAULT_CLAIM_TTL_SECONDS, assert_claim_lock, assert_run_id, atomic_claim, build_claim_lock,
    release_claim, worker_write_gated,
};
pub use config::KanbanConfig;
pub use error::{KanbanError, Result};
pub use events::{KanbanEvent, KanbanEventKind, insert_event_sql};
pub use paths::{
    kanban_db_path, kanban_log_stderr, kanban_log_stdout, kanban_logs_dir, kanban_skills_dir,
    kanban_workspace_for, kanban_workspaces_root, validate_dir_workspace,
};
pub use dispatcher::{
    DispatcherContext, StrandedReport, StrandedSeverity, diagnose_stranded, run_dispatch_loop,
    run_dispatch_tick,
};
pub use notifier::{
    NotifierContext, NotifierTickReport, SendFn, run_notifier_loop, run_notifier_tick,
};
pub use pid::is_pid_alive;
pub use skills_bundle::{
    KANBAN_ORCHESTRATOR_SKILL_MD, KANBAN_ORCHESTRATOR_SKILL_NAME, KANBAN_WORKER_SKILL_MD,
    KANBAN_WORKER_SKILL_NAME, SyncAction, SyncReport, restore_bundled_kanban_skill,
    sync_bundled_kanban_skills,
};
pub use store::{CreateTaskOptions, KanbanStore, ListFilters};
// Phase 36.3.7.5 BUG-36.3.7.5-06.
pub use store_writer_impl::KanbanStoreWriterImpl;
pub use types::{
    KanbanStatus, KanbanWorkerSpec, Subscription, SwarmGraphIds, SwarmGraphSpec, Task,
    TaskComment, TaskLink, TaskRun,
};
pub use worker_spawn::{SAFE_SYSTEM_VARS, build_kanban_worker_env, spawn_worker};
pub use tools::{register_kanban_tools, KanbanSwarmTool};

/// Canonical KANBAN_GUIDANCE system-prompt block injected into worker
/// prompts when `HERMES_KANBAN_TASK` is set at process start (D-26).
///
/// Sourced from upstream `agent/prompt_builder.py` and
/// `kanban-worker-skill-upstream.md` preamble (Plan 05).
/// See `kanban_guidance` module for the full constant and cache-stability doc.
pub use kanban_guidance::KANBAN_GUIDANCE;
