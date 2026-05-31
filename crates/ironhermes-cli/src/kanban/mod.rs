//! Kanban board CLI subcommand (Phase 36.3.7, D-33/D-34/D-35).
//!
//! Mirrors the `cron` module structure: a `KanbanCommands` clap-derive enum
//! plus a `handle_kanban_command` dispatcher that routes to per-verb
//! implementations in `commands.rs`.
//!
//! All verbs route through `KanbanStore` — no raw SQL in this module.

pub mod commands;
pub mod format;
pub mod store_reader_impl;

pub use store_reader_impl::KanbanStoreReaderImpl;
// Phase 36.3.7.5 BUG-36.3.7.5-06: KanbanStoreWriterImpl is published from
// ironhermes-kanban (gateway needs it; gateway has no dep on ironhermes-cli;
// reverse dep is circular). Re-export here for ergonomic discoverability.
pub use ironhermes_kanban::KanbanStoreWriterImpl;

use anyhow::Result;
use clap::Subcommand;

// ---------------------------------------------------------------------------
// KanbanCommands enum
// ---------------------------------------------------------------------------

/// Kanban board management commands (Phase 36.3.7 D-33).
///
/// Operator-facing (read + write):
///   init, create, list, show, assign, link, unlink, claim, comment,
///   complete, block, unblock, archive, tail, watch, runs, assignees,
///   dispatch, stats, log, context, gc
///
/// Operator-recovery:
///   reclaim, reassign, diagnostics, daemon
#[derive(Subcommand)]
pub enum KanbanCommands {
    /// Initialize the kanban DB at ~/.ironhermes/kanban.db
    #[command(name = "init")]
    Init,

    /// Create a new task
    #[command(name = "create")]
    Create {
        /// Task title
        title: String,
        /// Assignee profile name
        #[arg(long)]
        assignee: String,
        /// Task body/description
        #[arg(long)]
        body: Option<String>,
        /// Parent task IDs (repeatable)
        #[arg(long = "parent")]
        parents: Vec<String>,
        /// Tenant name
        #[arg(long)]
        tenant: Option<String>,
        /// Workspace spec (scratch | dir:<abs-path> | worktree)
        #[arg(long)]
        workspace: Option<String>,
        /// Skills to attach (repeatable)
        #[arg(long = "skill")]
        skills: Vec<String>,
        /// Task priority (default 0, higher = higher priority)
        #[arg(long)]
        priority: Option<i64>,
        /// Put task in triage status instead of ready
        #[arg(long)]
        triage: bool,
        /// Idempotency key — short-circuits to existing task if key already exists
        #[arg(long)]
        idempotency_key: Option<String>,
        /// Max runtime per attempt (e.g. "30m", "2h", or seconds)
        #[arg(long)]
        max_runtime: Option<String>,
        /// Max retries before circuit breaker trips
        #[arg(long)]
        max_retries: Option<i64>,
        /// Schedule start time (unix epoch float or ISO-like string)
        #[arg(long)]
        scheduled_at: Option<String>,
        /// Branch hint for worktree workspaces (stored as marker on task)
        #[arg(long)]
        branch: Option<String>,
        /// Output created task as JSON {task_id, status, assignee}
        #[arg(long)]
        json: bool,
    },

    /// List tasks on the board
    #[command(name = "list")]
    List {
        /// Filter to your own profile (HERMES_PROFILE env)
        #[arg(long)]
        mine: bool,
        /// Filter by assignee
        #[arg(long)]
        assignee: Option<String>,
        /// Filter by status (triage|todo|ready|running|blocked|done|archived)
        #[arg(long)]
        status: Option<String>,
        /// Filter by tenant
        #[arg(long)]
        tenant: Option<String>,
        /// Include archived tasks
        #[arg(long)]
        archived: bool,
        /// Output as JSON array
        #[arg(long)]
        json: bool,
    },

    /// Show full details for a task
    #[command(name = "show")]
    Show {
        /// Task ID
        id: String,
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },

    /// Assign a task to a profile (use "none" to unassign)
    #[command(name = "assign")]
    Assign {
        /// Task ID
        id: String,
        /// Profile name (or "none" to unassign)
        profile: String,
    },

    /// Link a child task to a parent
    #[command(name = "link")]
    Link {
        /// Parent task ID
        parent_id: String,
        /// Child task ID
        child_id: String,
    },

    /// Remove a parent→child dependency link
    #[command(name = "unlink")]
    Unlink {
        /// Parent task ID
        parent_id: String,
        /// Child task ID
        child_id: String,
    },

    /// Manually claim a task (operator debugging)
    #[command(name = "claim")]
    Claim {
        /// Task ID
        id: String,
        /// Claim TTL in seconds (default: 900)
        #[arg(long)]
        ttl: Option<u64>,
    },

    /// Add a comment to a task
    #[command(name = "comment")]
    Comment {
        /// Task ID
        id: String,
        /// Comment body
        body: String,
        /// Author name (default: HERMES_PROFILE env or "operator")
        #[arg(long)]
        author: Option<String>,
    },

    /// Mark one or more tasks complete
    ///
    /// D-34: bulk complete with --summary or --metadata is REFUSED (exit code 2).
    /// Use individual calls or omit summary/metadata for bulk admin-close.
    #[command(name = "complete")]
    Complete {
        /// Task ID(s) to complete
        ids: Vec<String>,
        /// Outcome label (e.g. "success", "partial")
        #[arg(long)]
        result: Option<String>,
        /// Handoff summary (per-run; refused if multiple ids given)
        #[arg(long)]
        summary: Option<String>,
        /// Free-form JSON metadata dict (per-run; refused if multiple ids given)
        #[arg(long)]
        metadata: Option<String>,
    },

    /// Block a task with a reason
    #[command(name = "block")]
    Block {
        /// Primary task ID
        id: String,
        /// Block reason
        reason: String,
        /// Additional task IDs to block
        #[arg(long = "id")]
        extra_ids: Vec<String>,
    },

    /// Unblock one or more tasks (return to ready)
    #[command(name = "unblock")]
    Unblock {
        /// Task ID(s) to unblock
        ids: Vec<String>,
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },

    /// Signal worker liveness during long operations (Phase 36.3.7.6 D-cli-heartbeat-parity).
    ///
    /// Appends a `heartbeat` event row to `task_events`. Workers running > 1 hour
    /// should call this at least hourly to avoid dispatcher staleness reclaim.
    #[command(name = "heartbeat")]
    Heartbeat {
        /// Task id to heartbeat.
        id: String,
        /// Optional free-form progress note.
        #[arg(long)]
        note: Option<String>,
    },

    /// Create an atomic multi-card swarm graph for orchestrator fan-out (Phase 36.3.7.7).
    ///
    /// Materializes a root + N worker cards + optional verifier + optional
    /// synthesizer + optional blackboard comment in one BEGIN IMMEDIATE
    /// transaction. See docs/kanban/reference.md §664 for canonical semantics
    /// and graph topology shapes (P1 fan-out, fan-out+verify, full 4-tier,
    /// P3 quorum).
    #[command(name = "swarm")]
    Swarm {
        /// Shared goal / task description for the entire swarm.
        goal: String,
        /// Worker assignees, comma-separated (mutually exclusive with --workers-json).
        #[arg(long, conflicts_with = "workers_json")]
        workers: Option<String>,
        /// Worker specs as JSON array `[{assignee, title?, body?}, ...]` (mutually exclusive with --workers).
        #[arg(long, conflicts_with = "workers")]
        workers_json: Option<String>,
        /// Verifier assignee (optional). When present, verifier card's parents are all worker IDs.
        #[arg(long)]
        verifier: Option<String>,
        /// Synthesizer assignee (optional). Parents are [verifier] when verifier present, else all workers.
        #[arg(long)]
        synthesizer: Option<String>,
        /// Blackboard JSON to seed as comment on root card (mutually exclusive with --blackboard-file).
        #[arg(long, conflicts_with = "blackboard_file")]
        blackboard: Option<String>,
        /// Path to JSON file for blackboard (mutually exclusive with --blackboard).
        #[arg(long, conflicts_with = "blackboard")]
        blackboard_file: Option<std::path::PathBuf>,
        /// Shared workspace spec for all cards (scratch | dir:<abs-path> | worktree).
        #[arg(long)]
        workspace: Option<String>,
        /// Shared skills to attach (repeatable).
        #[arg(long = "skill")]
        skills: Vec<String>,
        /// Shared tenant id across all cards.
        #[arg(long)]
        tenant: Option<String>,
        /// Shared priority across all cards (default 0).
        #[arg(long)]
        priority: Option<i64>,
        /// Shared max runtime per attempt (e.g. "30m", "2h", or seconds).
        #[arg(long)]
        max_runtime: Option<String>,
        /// Shared max retries cap.
        #[arg(long)]
        max_retries: Option<i64>,
        /// Idempotency key — per-card suffixes derive as k:root / k:worker:{i} / k:verifier / k:synthesizer.
        #[arg(long)]
        idempotency_key: Option<String>,
        /// Output result as JSON {root_id, worker_ids, verifier_id, synthesizer_id, blackboard_event_id}.
        #[arg(long)]
        json: bool,
    },

    /// Fan-out @mention delegations from a task body as child tasks (Phase 36.3.7.8).
    ///
    /// Parses all `@handle` occurrences from the task's body (or `--body-override`),
    /// resolves each handle against known assignees, and materializes child tasks
    /// in one atomic transaction. See docs/kanban/reference.md §741 for semantics.
    #[command(name = "mention")]
    Mention {
        /// Source task ID (defaults to $HERMES_KANBAN_TASK in worker mode).
        task_id: Option<String>,
        /// Fallback policy for unknown handles: skip | pending | error (default: skip).
        #[arg(long, default_value = "skip")]
        fallback_policy: String,
        /// Idempotency key — re-invocation returns existing children.
        #[arg(long)]
        idempotency_key: Option<String>,
        /// Parse this string instead of the task's current body (preview mode).
        #[arg(long)]
        body_override: Option<String>,
        /// Output result as JSON.
        #[arg(long)]
        json: bool,
    },

    /// Archive one or more tasks
    #[command(name = "archive")]
    Archive {
        /// Task ID(s) to archive
        ids: Vec<String>,
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },

    /// Tail events for a specific task (Ctrl-C to stop)
    #[command(name = "tail")]
    Tail {
        /// Task ID
        id: String,
    },

    /// Watch board-wide events (Ctrl-C to stop)
    #[command(name = "watch")]
    Watch {
        /// Filter by assignee
        #[arg(long)]
        assignee: Option<String>,
        /// Filter by tenant
        #[arg(long)]
        tenant: Option<String>,
        /// Comma-separated event kinds to show
        #[arg(long)]
        kinds: Option<String>,
        /// Poll interval in seconds (default: 2)
        #[arg(long)]
        interval: Option<u64>,
    },

    /// Show run history for a task
    #[command(name = "runs")]
    Runs {
        /// Task ID
        id: String,
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },

    /// List all assignees and their task counts
    #[command(name = "assignees")]
    Assignees {
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },

    /// Run one dispatcher tick (one-shot; gates on DB)
    #[command(name = "dispatch")]
    Dispatch {
        /// Preview what would be claimed (not yet implemented; logs a warning)
        #[arg(long)]
        dry_run: bool,
        /// Maximum tasks to claim this tick
        #[arg(long)]
        max: Option<usize>,
        /// Circuit breaker failure limit override
        #[arg(long)]
        failure_limit: Option<usize>,
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },

    /// Show board statistics (task counts by status)
    #[command(name = "stats")]
    Stats {
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },

    /// Show stdout/stderr logs for a task
    #[command(name = "log")]
    Log {
        /// Task ID
        id: String,
        /// Show only the last N bytes of each log file
        #[arg(long)]
        tail: Option<u64>,
    },

    /// Show the worker context envelope for a task
    #[command(name = "context")]
    Context {
        /// Task ID
        id: String,
    },

    /// Garbage-collect old events and log files
    #[command(name = "gc")]
    Gc {
        /// Delete events older than N days (default: 30)
        #[arg(long)]
        event_retention_days: Option<u64>,
        /// Delete log files older than N days (default: 30)
        #[arg(long)]
        log_retention_days: Option<u64>,
    },

    // ---------------------------------------------------------------------------
    // Operator-recovery verbs (D-33)
    // ---------------------------------------------------------------------------

    /// Force-release a task's claim (operator emergency)
    #[command(name = "reclaim")]
    Reclaim {
        /// Task ID
        id: String,
    },

    /// Reassign a task (optionally releasing its current claim)
    #[command(name = "reassign")]
    Reassign {
        /// Task ID
        id: String,
        /// New profile name
        new_profile: String,
        /// Release existing claim before reassigning
        #[arg(long)]
        reclaim: bool,
    },

    /// Show stranded-task diagnostic report
    #[command(name = "diagnostics")]
    Diagnostics {
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },

    /// Run the dispatcher as a long-lived process (DEPRECATED — use gateway instead)
    #[command(name = "daemon")]
    Daemon {
        /// Required flag to acknowledge deprecated usage
        #[arg(long)]
        force: bool,
        /// Circuit breaker failure limit override
        #[arg(long)]
        failure_limit: Option<usize>,
        /// Path to write PID file (removed on exit)
        #[arg(long)]
        pidfile: Option<String>,
    },

    // ---------------------------------------------------------------------------
    // Phase 36.3.7.5 — notifier subscriptions (BUG-36.3.7.5-05)
    // ---------------------------------------------------------------------------

    /// Subscribe a chat to a task's terminal events (gateway bridge hook)
    #[command(name = "notify-subscribe")]
    NotifySubscribe {
        /// Task ID
        task_id: String,
        /// Platform name (telegram | discord | slack | ...)
        #[arg(long)]
        platform: String,
        /// Chat ID on the platform
        #[arg(long = "chat-id")]
        chat_id: String,
        /// Optional thread ID (Telegram super-group topic, Discord thread)
        #[arg(long = "thread-id")]
        thread_id: Option<String>,
        /// Optional user ID (informational; not stored separately in v1)
        #[arg(long = "user-id")]
        user_id: Option<String>,
    },

    /// List subscriptions (all, or filtered by task)
    #[command(name = "notify-list")]
    NotifyList {
        /// Optional task ID to filter by
        task_id: Option<String>,
        /// Output as JSON array
        #[arg(long)]
        json: bool,
    },

    /// Remove subscription(s) — all for a task by default, optionally filtered
    #[command(name = "notify-unsubscribe")]
    NotifyUnsubscribe {
        /// Task ID
        task_id: String,
        /// Optional platform filter
        #[arg(long)]
        platform: Option<String>,
        /// Optional chat-id filter
        #[arg(long = "chat-id")]
        chat_id: Option<String>,
        /// Optional thread-id filter (informational; v1 ignores for filtering)
        #[arg(long = "thread-id")]
        thread_id: Option<String>,
    },
}

// ---------------------------------------------------------------------------
// handle_kanban_command
// ---------------------------------------------------------------------------

/// Dispatch a `KanbanCommands` variant to the appropriate handler.
///
/// Returns an exit code (0 = success, 1 = error, 2 = D-34 bulk refusal).
pub async fn handle_kanban_command(cmd: KanbanCommands) -> anyhow::Result<i32> {
    match cmd {
        KanbanCommands::Init => commands::cmd_init().await,
        KanbanCommands::Create {
            title,
            assignee,
            body,
            parents,
            tenant,
            workspace,
            skills,
            priority,
            triage,
            idempotency_key,
            max_runtime,
            max_retries,
            scheduled_at,
            branch,
            json,
        } => {
            commands::cmd_create(
                title, assignee, body, parents, tenant, workspace, skills,
                priority, triage, idempotency_key, max_runtime, max_retries,
                scheduled_at, branch, json,
            )
            .await
        }
        KanbanCommands::List {
            mine,
            assignee,
            status,
            tenant,
            archived,
            json,
        } => commands::cmd_list(mine, assignee, status, tenant, archived, json).await,
        KanbanCommands::Show { id, json } => commands::cmd_show(id, json).await,
        KanbanCommands::Assign { id, profile } => commands::cmd_assign(id, profile).await,
        KanbanCommands::Link {
            parent_id,
            child_id,
        } => commands::cmd_link(parent_id, child_id).await,
        KanbanCommands::Unlink {
            parent_id,
            child_id,
        } => commands::cmd_unlink(parent_id, child_id).await,
        KanbanCommands::Claim { id, ttl } => commands::cmd_claim(id, ttl).await,
        KanbanCommands::Comment { id, body, author } => {
            commands::cmd_comment(id, body, author).await
        }
        KanbanCommands::Complete {
            ids,
            result,
            summary,
            metadata,
        } => commands::cmd_complete(ids, result, summary, metadata).await,
        KanbanCommands::Block {
            id,
            reason,
            extra_ids,
        } => commands::cmd_block(id, reason, extra_ids).await,
        KanbanCommands::Unblock { ids, json } => commands::cmd_unblock(ids, json).await,
        // Phase 36.3.7.6 D-cli-heartbeat-parity — closes reference.md:583 doc-vs-impl gap.
        KanbanCommands::Heartbeat { id, note } => commands::cmd_heartbeat(id, note).await,
        // Phase 36.3.7.7 BUG-36.3.7.7-01 — kanban swarm (D-topology-shapes).
        KanbanCommands::Swarm {
            goal,
            workers,
            workers_json,
            verifier,
            synthesizer,
            blackboard,
            blackboard_file,
            workspace,
            skills,
            tenant,
            priority,
            max_runtime,
            max_retries,
            idempotency_key,
            json,
        } => {
            commands::cmd_swarm(
                goal,
                workers,
                workers_json,
                verifier,
                synthesizer,
                blackboard,
                blackboard_file,
                workspace,
                skills,
                tenant,
                priority,
                max_runtime,
                max_retries,
                idempotency_key,
                json,
            )
            .await
        }
        // Phase 36.3.7.8 — kanban_mention (@mention delegation parser, inline routing).
        KanbanCommands::Mention {
            task_id,
            fallback_policy,
            idempotency_key,
            body_override,
            json,
        } => {
            commands::cmd_mention(
                task_id,
                fallback_policy,
                idempotency_key,
                body_override,
                json,
            )
            .await
        }
        KanbanCommands::Archive { ids, json } => commands::cmd_archive(ids, json).await,
        KanbanCommands::Tail { id } => commands::cmd_tail(id).await,
        KanbanCommands::Watch {
            assignee,
            tenant,
            kinds,
            interval,
        } => commands::cmd_watch(assignee, tenant, kinds, interval).await,
        KanbanCommands::Runs { id, json } => commands::cmd_runs(id, json).await,
        KanbanCommands::Assignees { json } => commands::cmd_assignees(json).await,
        KanbanCommands::Dispatch {
            dry_run,
            max,
            failure_limit,
            json,
        } => commands::cmd_dispatch(dry_run, max, failure_limit, json).await,
        KanbanCommands::Stats { json } => commands::cmd_stats(json).await,
        KanbanCommands::Log { id, tail } => commands::cmd_log(id, tail).await,
        KanbanCommands::Context { id } => commands::cmd_context(id).await,
        KanbanCommands::Gc {
            event_retention_days,
            log_retention_days,
        } => commands::cmd_gc(event_retention_days, log_retention_days).await,
        KanbanCommands::Reclaim { id } => commands::cmd_reclaim(id).await,
        KanbanCommands::Reassign {
            id,
            new_profile,
            reclaim,
        } => commands::cmd_reassign(id, new_profile, reclaim).await,
        KanbanCommands::Diagnostics { json } => commands::cmd_diagnostics(json).await,
        KanbanCommands::Daemon {
            force: _,
            failure_limit,
            pidfile,
        } => commands::cmd_daemon(failure_limit, pidfile).await,
        // ---------------------------------------------------------------------
        // Phase 36.3.7.5 BUG-36.3.7.5-05 — notifier subscriptions
        // ---------------------------------------------------------------------
        KanbanCommands::NotifySubscribe {
            task_id,
            platform,
            chat_id,
            thread_id,
            user_id: _,
        } => commands::cmd_notify_subscribe(task_id, platform, chat_id, thread_id).await,
        KanbanCommands::NotifyList { task_id, json } => {
            commands::cmd_notify_list(task_id, json).await
        }
        KanbanCommands::NotifyUnsubscribe {
            task_id,
            platform,
            chat_id,
            thread_id: _,
        } => commands::cmd_notify_unsubscribe(task_id, platform, chat_id).await,
    }
}
