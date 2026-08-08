//! Per-verb implementations for the `kanban` CLI subcommand.
//!
//! Each `cmd_<verb>` function opens `open_store_for_board(board)` and
//! calls the appropriate store method — no raw SQL in this file.
//!
//! Return value: `Result<i32>` where the integer is the process exit code
//! (0 = success, 1 = error, 2 = D-34 bulk-summary refusal).

use anyhow::{Context, Result, anyhow};
use ironhermes_kanban::cas::{atomic_claim, build_claim_lock, release_claim};
use ironhermes_kanban::decomposer::{
    ChildSpec, DecomposeFn, DecomposeOutput, DecomposeRequest, decompose_triage_task,
    specify_triage_task,
};
use ironhermes_kanban::store::{CreateTaskOptions, ListFilters};
use ironhermes_kanban::{
    DispatcherContext, KanbanConfig, KanbanStore, KanbanWorkerSpec, StrandedReport, SwarmGraphSpec,
    diagnose_stranded, run_dispatch_tick,
};
use std::sync::Arc;
use tokio::sync::Mutex as TokioMutex;

use ironhermes_core::config::Config;
use ironhermes_kanban::types::TaskComment;

use super::format::{
    format_assignees, format_diagnostics, format_event_line, format_runs_table, format_stats,
    format_task_detail, format_task_json, format_task_table,
};

// ---------------------------------------------------------------------------
// Helper: open the correct board store (board-aware, D-02 4-tier resolver)
// ---------------------------------------------------------------------------

/// Open the correct board store given an optional `--board <slug>` flag value.
///
/// When `board` is `Some(slug)`, validates the slug (T-1 path-traversal guard)
/// then opens that board directly.
/// When `board` is `None`, runs the 4-tier resolver (D-02) to determine the
/// active board and opens it.
///
/// All existing `cmd_*` functions call this helper (Plan 04 retrofit).
/// Also used by `boards.rs` functions.
pub fn open_store_for_board(board: Option<&str>) -> Result<KanbanStore> {
    match board {
        Some(raw_slug) => {
            // T-1: validate slug at CLI boundary before constructing any path.
            let slug = ironhermes_kanban::board::validate_board_slug(raw_slug)
                .map_err(|e| anyhow::anyhow!("invalid --board slug: {}", e))?;
            // Honor IRONHERMES_KANBAN_DB (set by the dispatcher into the worker env) so a
            // worker spawned under a profile home still opens the dispatcher's tracked DB,
            // not a profile-scoped one where the task does not exist. Matches the LLM tools'
            // KanbanStore::open_from_env_or_board path (tools/complete.rs).
            KanbanStore::open_from_env_or_board(Some(&slug)).context("Failed to open board DB")
        }
        None => {
            let ctx = ironhermes_kanban::board::resolve_board_context(None)
                .context("Failed to resolve board context")?;
            // See above: honor IRONHERMES_KANBAN_DB before the profile-home-derived path.
            KanbanStore::open_from_env_or_board(Some(&ctx.slug)).context("Failed to open kanban DB")
        }
    }
}

fn profile_from_env() -> String {
    std::env::var("HERMES_PROFILE").unwrap_or_else(|_| "operator".to_string())
}

/// Fetch all comments for a task — not yet on KanbanStore public API; use raw conn.
fn get_comments(store: &KanbanStore, task_id: &str) -> Result<Vec<TaskComment>> {
    let mut stmt = store.conn.prepare(
        "SELECT id, task_id, author, body, created_at \
         FROM task_comments WHERE task_id = ?1 ORDER BY created_at",
    )?;
    let rows = stmt.query_map(rusqlite::params![task_id], |row| {
        Ok(TaskComment {
            id: row.get(0)?,
            task_id: row.get(1)?,
            author: row.get(2)?,
            body: row.get(3)?,
            created_at: row.get(4)?,
        })
    })?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r?);
    }
    Ok(out)
}

/// Delete a parent→child link — not yet on KanbanStore public API.
fn delete_link(store: &mut KanbanStore, parent_id: &str, child_id: &str) -> Result<()> {
    store.conn.execute(
        "DELETE FROM task_links WHERE parent_id = ?1 AND child_id = ?2",
        rusqlite::params![parent_id, child_id],
    )?;
    Ok(())
}

// ---------------------------------------------------------------------------
// cmd_init
// ---------------------------------------------------------------------------

pub async fn cmd_init(board: Option<&str>) -> Result<i32> {
    // Resolve the path first for display, then open (which initializes the schema).
    let path = match board {
        Some(slug) => ironhermes_kanban::paths::board_db_path_for_slug(slug),
        None => {
            let ctx = ironhermes_kanban::board::resolve_board_context(None)
                .context("Failed to resolve board context")?;
            ctx.db_path
        }
    };
    open_store_for_board(board).context("Failed to initialize kanban DB")?;
    println!("Initialized kanban DB at {}", path.display());
    Ok(0)
}

// ---------------------------------------------------------------------------
// cmd_create
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
pub async fn cmd_create(
    title: String,
    assignee: String,
    body: Option<String>,
    parents: Vec<String>,
    tenant: Option<String>,
    workspace: Option<String>,
    skills: Vec<String>,
    priority: Option<i64>,
    triage: bool,
    goal: bool,
    goal_max_turns: u32,
    goal_toolset: Option<String>,
    idempotency_key: Option<String>,
    max_runtime: Option<String>,
    max_retries: Option<i64>,
    scheduled_at: Option<String>,
    branch: Option<String>,
    json: bool,
    board: Option<&str>,
) -> Result<i32> {
    let mut store = open_store_for_board(board)?;

    // Parse max_runtime (e.g. "30m", "3600")
    let max_runtime_seconds: Option<i64> = max_runtime
        .as_deref()
        .map(parse_duration_secs)
        .transpose()?;

    // Handle --branch: append as marker to body
    let effective_body = match (body, branch) {
        (Some(b), Some(br)) => Some(format!("{}\n\nbranch: {}", b, br)),
        (None, Some(br)) => Some(format!("branch: {}", br)),
        (b, None) => b,
    };

    let opts = CreateTaskOptions {
        body: effective_body,
        parents,
        tenant,
        workspace,
        skills: if skills.is_empty() {
            None
        } else {
            Some(skills)
        },
        priority,
        idempotency_key,
        scheduled_at: scheduled_at.as_deref().map(parse_timestamp).transpose()?,
        max_runtime_seconds,
        max_retries,
        triage,
        created_by: Some(profile_from_env()),
        // Phase 36.3.7.12 Plan 03 D-01: forward --goal / --goal-max-turns to
        // the producer. The store coerces goal_max_turns = 0 → 20 (Plan 01
        // D-03 producer-level coercion) so passing the raw clap value
        // (default_value_t = 20) preserves D-03 even if a caller passes 0.
        goal_mode: goal,
        goal_max_turns,
        // Phase 36.3.7.13 F-03 / D-G1: --goal-toolset forwarded verbatim to
        // the store (None → NULL in DB → worker warns + skips filter per D-E2).
        // clap requires = "goal" enforces --goal-toolset only when --goal is set.
        goal_toolset,
    };

    let task = store
        .create_task(&title, &assignee, opts)
        .context("Failed to create task")?;

    // Phase 46.5-04 D-06: auto-subscribe the operator's configured
    // default_notify target (source="auto") for this CLI (non-chat-origin)
    // creation path, mirroring the existing chat-origin auto-subscribe
    // hook. Non-fatal — the task was already created successfully; a
    // subscribe failure must not fail `hermes kanban create`.
    //
    // SECURITY (T-46.5-20): the target comes exclusively from
    // KanbanConfig.default_notify (operator config) — never from any of
    // this command's task-controlled args (title/body/assignee/etc).
    let main_config = Config::load().unwrap_or_default();
    let kanban_config = load_kanban_config(&main_config);
    if let Some(ref target) = kanban_config.default_notify
        && let Err(e) = ironhermes_kanban::subscribe_default_notify(&mut store, target, &task.id)
    {
        eprintln!(
            "warning: failed to auto-subscribe default_notify target for {}: {}",
            task.id, e
        );
    }

    if json {
        println!(
            "{}",
            serde_json::json!({
                "task_id": task.id,
                "status": task.status,
                "assignee": task.assignee,
            })
        );
    } else {
        println!("{}", task.id);
    }
    Ok(0)
}

/// Parse a duration string like "30s", "30m", "2h", "1d", or "3600" into seconds.
///
/// CR-01 (Phase 36.3.7.7 code review): kept aligned with the tool-surface
/// `parse_max_runtime` (in `crates/ironhermes-kanban/src/tools/create.rs`)
/// so CLI <-> tool stay byte-for-byte compatible — `--max-runtime 30s` and
/// `--max-runtime 1d` are now accepted at the CLI surface, matching the tool.
fn parse_duration_secs(s: &str) -> Result<i64> {
    let trimmed = s.trim();
    if trimmed.is_empty() {
        return Err(anyhow!("empty duration"));
    }
    let split_pos = trimmed.find(|c: char| !c.is_ascii_digit());
    match split_pos {
        None => trimmed
            .parse::<i64>()
            .context("Invalid duration (use e.g. '30s', '30m', '2h', '1d', or seconds)"),
        Some(pos) => {
            let (num_str, suffix) = trimmed.split_at(pos);
            let n: i64 = num_str
                .parse()
                .context("Invalid numeric prefix in duration")?;
            match suffix {
                "s" => Ok(n),
                "m" => Ok(n * 60),
                "h" => Ok(n * 3600),
                "d" => Ok(n * 86400),
                other => Err(anyhow!(
                    "Unknown duration suffix '{other}' (expected s/m/h/d)"
                )),
            }
        }
    }
}

/// Parse an ISO-like timestamp or unix epoch string into epoch float.
fn parse_timestamp(s: &str) -> Result<f64> {
    // Try as plain float first
    if let Ok(f) = s.parse::<f64>() {
        return Ok(f);
    }
    Err(anyhow!(
        "Could not parse timestamp '{}' — use a unix epoch float",
        s
    ))
}

// ---------------------------------------------------------------------------
// cmd_list
// ---------------------------------------------------------------------------

pub async fn cmd_list(
    mine: bool,
    assignee: Option<String>,
    status: Option<String>,
    tenant: Option<String>,
    archived: bool,
    json: bool,
    board: Option<&str>,
) -> Result<i32> {
    let store = open_store_for_board(board)?;

    let effective_assignee = if mine {
        Some(profile_from_env())
    } else {
        assignee
    };

    let filters = ListFilters {
        assignee: effective_assignee,
        status,
        tenant,
        archived,
        limit: None,
    };

    let tasks = store.list_tasks(filters).context("Failed to list tasks")?;

    if json {
        println!("{}", serde_json::to_string_pretty(&tasks)?);
    } else {
        print!("{}", format_task_table(&tasks));
    }
    Ok(0)
}

// ---------------------------------------------------------------------------
// cmd_show
// ---------------------------------------------------------------------------

pub async fn cmd_show(id: String, json: bool, board: Option<&str>) -> Result<i32> {
    let store = open_store_for_board(board)?;
    let task = store.get_task(&id).context("Task not found")?;
    let runs = store.get_runs(&id).context("Failed to load runs")?;
    let events = store.get_events(&id).context("Failed to load events")?;
    let comments = get_comments(&store, &id).unwrap_or_default();

    if json {
        println!("{}", format_task_json(&task, &runs, &events, &comments));
    } else {
        print!("{}", format_task_detail(&task, &runs, &events, &comments));
    }
    Ok(0)
}

// ---------------------------------------------------------------------------
// cmd_assign
// ---------------------------------------------------------------------------

pub async fn cmd_assign(id: String, profile: String, board: Option<&str>) -> Result<i32> {
    let mut store = open_store_for_board(board)?;
    // "none" sentinel → unassign (set to empty string)
    let assignee = if profile.eq_ignore_ascii_case("none") {
        ""
    } else {
        &profile
    };
    store
        .assign_task(&id, assignee)
        .context("Failed to assign task")?;
    println!("Assigned {} to '{}'", id, assignee);
    Ok(0)
}

// ---------------------------------------------------------------------------
// cmd_link / cmd_unlink
// ---------------------------------------------------------------------------

pub async fn cmd_link(parent_id: String, child_id: String, board: Option<&str>) -> Result<i32> {
    let mut store = open_store_for_board(board)?;
    store
        .insert_link(&parent_id, &child_id)
        .context("Failed to create link")?;
    println!("Linked {} → {}", parent_id, child_id);
    Ok(0)
}

pub async fn cmd_unlink(parent_id: String, child_id: String, board: Option<&str>) -> Result<i32> {
    let mut store = open_store_for_board(board)?;
    delete_link(&mut store, &parent_id, &child_id).context("Failed to delete link")?;
    println!("Unlinked {} → {}", parent_id, child_id);
    Ok(0)
}

// ---------------------------------------------------------------------------
// cmd_claim
// ---------------------------------------------------------------------------

pub async fn cmd_claim(id: String, ttl: Option<u64>, board: Option<&str>) -> Result<i32> {
    let mut store = open_store_for_board(board)?;
    let hostname = ironhermes_kanban::pid::current_hostname();
    let pid = std::process::id();
    let lock = build_claim_lock(&hostname, pid);
    let ttl_secs = ttl.unwrap_or(ironhermes_kanban::DEFAULT_CLAIM_TTL_SECONDS);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64();
    let run_id = format!("run_{}", uuid::Uuid::new_v4().simple());

    let won = atomic_claim(&mut store.conn, &id, &lock, pid, ttl_secs, now, &run_id)
        .context("Failed to claim task")?;

    if won {
        println!("Claimed {} — run_id: {} lock: {}", id, run_id, lock);
    } else {
        eprintln!("Failed to claim {} (already claimed or not ready)", id);
        return Ok(1);
    }
    Ok(0)
}

// ---------------------------------------------------------------------------
// cmd_comment
// ---------------------------------------------------------------------------

pub async fn cmd_comment(
    id: String,
    body: String,
    author: Option<String>,
    board: Option<&str>,
) -> Result<i32> {
    let mut store = open_store_for_board(board)?;
    let author = author.unwrap_or_else(profile_from_env);
    store
        .add_comment(&id, &author, &body)
        .context("Failed to add comment")?;
    println!("Comment added to {}", id);
    Ok(0)
}

// ---------------------------------------------------------------------------
// cmd_attach
// ---------------------------------------------------------------------------

/// Attach a local file to a task (D-04/D-03).
///
/// Converges on `KanbanStore::add_attachment` — the SAME traversal-safe
/// writer the web upload surface uses (Plan 03). This function only reads
/// the local file and derives its display filename; all storage (on-disk
/// path construction, path-traversal defense, DB row insert) lives in
/// `add_attachment`. No duplicated storage logic here.
pub async fn cmd_attach(id: String, file: std::path::PathBuf, board: Option<&str>) -> Result<i32> {
    let mut store = open_store_for_board(board)?;

    let file_name = file
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| anyhow!("attachment path has no file name: {}", file.display()))?
        .to_string();

    let bytes = std::fs::read(&file)
        .with_context(|| format!("read attachment file {}", file.display()))?;
    let size_bytes = bytes.len();

    let profile = profile_from_env();
    let meta = store
        .add_attachment(&id, &file_name, &bytes, None, Some(&profile))
        .context("Failed to attach file")?;

    println!(
        "Attached {} ({} bytes) to {} as {}",
        file_name, size_bytes, id, meta.id
    );
    Ok(0)
}

// ---------------------------------------------------------------------------
// cmd_complete
// ---------------------------------------------------------------------------

pub async fn cmd_complete(
    ids: Vec<String>,
    result: Option<String>,
    summary: Option<String>,
    metadata: Option<String>,
    output_path: Option<String>,
    board: Option<&str>,
) -> Result<i32> {
    // D-34: bulk complete with --summary/--metadata refused.
    // D-10: bulk complete with --output-path is also per-run and refused.
    if ids.len() > 1 && (summary.is_some() || metadata.is_some() || output_path.is_some()) {
        eprintln!(
            "bulk complete with --summary/--metadata/--output-path refused per D-34 — \
             handoff is per-run; use individual complete calls or omit \
             summary/metadata/output-path for bulk admin-close"
        );
        return Ok(2);
    }

    let mut store = open_store_for_board(board)?;
    let profile = profile_from_env();
    let metadata_val: Option<serde_json::Value> = metadata
        .as_deref()
        .map(|s| serde_json::from_str(s).context("--metadata must be valid JSON"))
        .transpose()?;

    let mut any_err = false;
    for id in &ids {
        match store.complete_task(
            id,
            summary.as_deref(),
            metadata_val.as_ref(),
            result.as_deref(),
            None,
            None,
            &profile,
            output_path.as_deref(),
        ) {
            Ok(()) => println!("{}: completed", id),
            Err(e) => {
                eprintln!("{}: error: {}", id, e);
                any_err = true;
            }
        }
    }

    Ok(if any_err { 1 } else { 0 })
}

// ---------------------------------------------------------------------------
// cmd_block
// ---------------------------------------------------------------------------

pub async fn cmd_block(
    id: String,
    reason: String,
    extra_ids: Vec<String>,
    board: Option<&str>,
) -> Result<i32> {
    let mut store = open_store_for_board(board)?;
    let mut all_ids = vec![id];
    all_ids.extend(extra_ids);

    let mut any_err = false;
    for id in &all_ids {
        match store.block_task(id, &reason, None) {
            Ok(()) => println!("{}: blocked", id),
            Err(e) => {
                eprintln!("{}: error: {}", id, e);
                any_err = true;
            }
        }
    }
    Ok(if any_err { 1 } else { 0 })
}

// ---------------------------------------------------------------------------
// cmd_unblock
// ---------------------------------------------------------------------------

pub async fn cmd_unblock(ids: Vec<String>, _json: bool, board: Option<&str>) -> Result<i32> {
    let mut store = open_store_for_board(board)?;
    let mut any_err = false;
    for id in &ids {
        match store.unblock_task(id) {
            Ok(()) => println!("{}: unblocked", id),
            Err(e) => {
                eprintln!("{}: error: {}", id, e);
                any_err = true;
            }
        }
    }
    Ok(if any_err { 1 } else { 0 })
}

// ---------------------------------------------------------------------------
// cmd_heartbeat (Phase 36.3.7.6 D-cli-heartbeat-parity)
// ---------------------------------------------------------------------------

/// Append a `heartbeat` event row for a task. Mirrors the kanban_heartbeat
/// LLM tool's append-event path; the dispatcher's staleness reader at
/// reference.md:869 is the single consumer of these rows.
pub async fn cmd_heartbeat(id: String, note: Option<String>, board: Option<&str>) -> Result<i32> {
    use ironhermes_kanban::KanbanEventKind;

    let mut store = open_store_for_board(board)?;
    let run_id_env = ironhermes_kanban::kanban_env("RUN_ID");
    let payload = note.as_ref().map(|n| serde_json::json!({ "note": n }));

    match store.append_event(
        &id,
        run_id_env.as_deref(),
        KanbanEventKind::Heartbeat,
        payload.as_ref(),
    ) {
        Ok(event_id) => {
            println!("heartbeat sent for {id} (event_id={event_id})");
            Ok(0)
        }
        Err(e) => {
            eprintln!("error: {e}");
            Ok(1)
        }
    }
}

// ---------------------------------------------------------------------------
// cmd_swarm (Phase 36.3.7.7 BUG-36.3.7.7-01 D-cli-verb-shape)
// ---------------------------------------------------------------------------

/// Create an atomic multi-card swarm graph for orchestrator fan-out.
///
/// Resolves the `--workers` vs `--workers-json` union (mutually exclusive at
/// parse time via clap `conflicts_with`; at least one must be present —
/// runtime check), resolves the `--blackboard` vs `--blackboard-file` union
/// (also conflicts_with; file is read to memory before building the spec per
/// Pitfall 6), builds a `SwarmGraphSpec`, and calls `store.create_swarm`.
///
/// Pretty output:
///   `Created swarm {root_id}: {N} workers[, verifier {vid}][, synth {sid}][ (+blackboard)]`
///
/// JSON output (`--json`): full `SwarmGraphIds` envelope.
#[allow(clippy::too_many_arguments)]
pub async fn cmd_swarm(
    goal: String,
    workers: Option<String>,
    workers_json: Option<String>,
    verifier: Option<String>,
    synthesizer: Option<String>,
    blackboard: Option<String>,
    blackboard_file: Option<std::path::PathBuf>,
    workspace: Option<String>,
    skills: Vec<String>,
    tenant: Option<String>,
    priority: Option<i64>,
    max_runtime: Option<String>,
    max_retries: Option<i64>,
    idempotency_key: Option<String>,
    json: bool,
    board: Option<&str>,
) -> Result<i32> {
    let mut store = open_store_for_board(board)?;

    // Worker normalization — Pitfall 5: clap does NOT comma-split; we do it here.
    //
    // WR-07 (Phase 36.3.7.7 code review): trim ONLY a single trailing comma
    // (forgiving "alice,bob," → 2 workers), but DO NOT filter empty middle
    // entries — let them flow through to validate_profile_name so
    // "alice,,bob" surfaces as a clear "invalid assignee" rejection
    // instead of silently producing 2 workers when the user typed 3.
    let worker_specs: Vec<KanbanWorkerSpec> = match (workers, workers_json) {
        (Some(flat), None) => {
            let stripped = flat.strip_suffix(',').unwrap_or(&flat);
            stripped
                .split(',')
                .map(|s| KanbanWorkerSpec {
                    assignee: s.trim().to_string(),
                    title: None,
                    body: None,
                })
                .collect()
        }
        (None, Some(rich)) => serde_json::from_str::<Vec<KanbanWorkerSpec>>(&rich)
            .context("Invalid --workers-json")?,
        (Some(_), Some(_)) => {
            return Err(anyhow!(
                "--workers and --workers-json are mutually exclusive"
            ));
        }
        (None, None) => return Err(anyhow!("--workers or --workers-json is required")),
    };

    if worker_specs.is_empty() {
        return Err(anyhow!("workers must contain at least one entry"));
    }

    // Blackboard normalization — Pitfall 6: file is read before building spec.
    let blackboard_value: Option<serde_json::Value> = match (blackboard, blackboard_file) {
        (Some(s), None) => Some(serde_json::from_str(&s).context("Invalid --blackboard JSON")?),
        (None, Some(path)) => {
            let file = std::fs::File::open(&path)
                .with_context(|| format!("Failed to open --blackboard-file {}", path.display()))?;
            Some(serde_json::from_reader(file).context("Invalid JSON in --blackboard-file")?)
        }
        (Some(_), Some(_)) => {
            return Err(anyhow!(
                "--blackboard and --blackboard-file are mutually exclusive"
            ));
        }
        (None, None) => None,
    };

    let max_runtime_seconds: Option<i64> = max_runtime
        .as_deref()
        .map(parse_duration_secs)
        .transpose()?;

    let created_by = profile_from_env();

    let spec = SwarmGraphSpec {
        goal,
        workers: worker_specs,
        verifier,
        synthesizer,
        blackboard: blackboard_value,
        workspace,
        skills: if skills.is_empty() {
            None
        } else {
            Some(skills)
        },
        tenant,
        priority,
        max_runtime_seconds,
        max_retries,
        idempotency_key,
        created_by: Some(created_by),
        body: None,
    };

    let ids = store.create_swarm(spec).context("Failed to create swarm")?;

    if json {
        println!("{}", serde_json::to_string_pretty(&ids)?);
    } else {
        let mut suffix_parts: Vec<String> = Vec::new();
        if let Some(ref vid) = ids.verifier_id {
            suffix_parts.push(format!("verifier {vid}"));
        }
        if let Some(ref sid) = ids.synthesizer_id {
            suffix_parts.push(format!("synth {sid}"));
        }
        let suffix = if suffix_parts.is_empty() {
            String::new()
        } else {
            format!(", {}", suffix_parts.join(", "))
        };
        let bb = if ids.blackboard_event_id.is_some() {
            " (+blackboard)"
        } else {
            ""
        };
        println!(
            "Created swarm {}: {} workers{}{}",
            ids.root_id,
            ids.worker_ids.len(),
            suffix,
            bb
        );
    }
    Ok(0)
}

// ---------------------------------------------------------------------------
// cmd_mention (Phase 36.3.7.8 — kanban_mention @mention delegation, inline routing)
//
// CRITICAL: runs the parser + resolver INLINE (NOT via the LLM tool registry).
// Plan 03's tool path is the LLM surface; this CLI path calls store.create_mention_children
// directly per the 36.3.7.6/36.3.7.7 precedent (PATTERNS.md Landmine 7 + RESEARCH §Pitfall 7).
// ---------------------------------------------------------------------------

/// Fan-out `@mention` delegations from a task body as child tasks.
///
/// Resolves handles inline (parser + resolver + store), NOT via the LLM tool
/// registry. Human operators invoke this verb directly; gateway `/kanban mention`
/// routes here via `DEFERRED_KANBAN_SUBVERBS` (REQ-08).
pub async fn cmd_mention(
    task_id: Option<String>,
    fallback_policy: String,
    idempotency_key: Option<String>,
    body_override: Option<String>,
    json: bool,
    board: Option<&str>,
) -> Result<i32> {
    use std::collections::HashSet;

    use ironhermes_kanban::mention::{
        FallbackPolicy, Resolution, ResolverCtx, SkipReason, parse_mentions, resolve_mention,
    };
    use ironhermes_kanban::store::{MentionChildSpec, MentionPlan};

    // 1. Resolve task_id from arg or $IRONHERMES_KANBAN_TASK env (dual-read via kanban_env).
    let task_id = task_id
        .or_else(|| ironhermes_kanban::kanban_env("TASK"))
        .ok_or_else(|| anyhow!("task_id required when IRONHERMES_KANBAN_TASK is not set"))?;

    // 2. Open store (sync handle; CLI is single-process).
    let mut store = open_store_for_board(board)?;

    // 3. Fetch the parent task.
    let parent = store
        .get_task(&task_id)
        .context("Failed to fetch parent task")?;

    // 4. Determine body to parse.
    let body = body_override.unwrap_or_else(|| parent.body.clone().unwrap_or_default());

    // 5. Run the parser (pure function — no IO).
    let spans = parse_mentions(&body);

    // 6. Build known-assignees HashSet via inline SELECT DISTINCT.
    //    Uses params![] — no string-interpolated SQL (T-36.3.7.8-04-02).
    //    CR-02 (Phase 36.3.7.8 code review): lowercase every DB-returned
    //    assignee so the resolver's case-insensitive contract holds even if
    //    a future migration or repair-script bypasses validate_profile_name.
    let known: HashSet<String> = {
        let mut stmt = store
            .conn
            .prepare(
                "SELECT DISTINCT assignee FROM tasks WHERE assignee != '' AND assignee IS NOT NULL",
            )
            .context("Failed to prepare known-assignees query")?;
        let rows = stmt
            .query_map(rusqlite::params![], |row| row.get::<_, String>(0))
            .context("Failed to query known assignees")?;
        let mut set = HashSet::new();
        for r in rows {
            let a = r.context("Failed to read assignee row")?;
            set.insert(a.to_lowercase());
        }
        set
    };

    // 7. Parse fallback_policy.
    let policy: FallbackPolicy = fallback_policy
        .parse()
        .context("Invalid --fallback-policy value (expected: skip | pending | error)")?;

    // 8. Build resolver context and resolve every span.
    // CR-02 (Phase 36.3.7.8 code review): lowercase parent.assignee to mirror
    // the LLM-tool path (tools/mention.rs:233) and honour the resolver
    // precondition documented at resolver.rs::ResolverCtx::parent_assignee
    // ("validated, lowercased").
    let parent_assignee = parent.assignee.to_lowercase();
    let ctx = ResolverCtx {
        parent_task_id: task_id.clone(),
        parent_assignee,
        known_assignees: &known,
        fallback_policy: policy,
    };

    let mut children: Vec<MentionChildSpec> = Vec::new();
    let mut skipped: Vec<serde_json::Value> = Vec::new();

    for (mention_index, span) in spans.iter().enumerate() {
        match resolve_mention(&span.handle, &ctx) {
            // WR-07 (Phase 36.3.7.8 code review): preserve original-case
            // `span.handle` in the child title (REQ-04 — MentionSpan.handle is
            // raw-cased prose). Mirrors tools/mention.rs:255,264 — operators
            // triaging child cards can see whether the prose said @Reviewer,
            // @reviewer, or @REVIEWER.
            Resolution::Resolved(assignee) => {
                children.push(MentionChildSpec {
                    handle: span.handle.clone(),
                    assignee,
                    mention_index,
                    title: format!("@{} mention from {}", span.handle, task_id),
                    body: None,
                });
            }
            Resolution::Fallback(assignee, _reason) => {
                children.push(MentionChildSpec {
                    handle: span.handle.clone(),
                    assignee,
                    mention_index,
                    title: format!("@{} mention from {}", span.handle, task_id),
                    body: None,
                });
            }
            Resolution::Skipped(reason) => {
                // Under Error policy, unknown-handle skips are hard errors.
                if matches!(policy, FallbackPolicy::Error)
                    && matches!(reason, SkipReason::UnknownHandle)
                {
                    // WR-05 (Phase 36.3.7.8 code review): when --json is set,
                    // emit a structured rejection envelope on stdout (mirroring
                    // the LLM tool's reject() shape) and exit non-zero so shell
                    // pipelines can parse the failure without scraping stderr.
                    if json {
                        println!(
                            "{}",
                            serde_json::to_string_pretty(&serde_json::json!({
                                "status": "rejected",
                                "reason": "unknown_handle",
                                "detail": format!(
                                    "handle '{}' did not resolve under fallback_policy=error",
                                    span.handle
                                ),
                            }))?
                        );
                        return Ok(2);
                    }
                    return Err(anyhow!(
                        "unknown_handle: handle '{}' did not resolve under fallback_policy=error",
                        span.handle
                    ));
                }
                // IN-04 (Phase 36.3.7.8 code review): emit snake_case reason
                // strings (matching the LLM-tool envelope) instead of Rust
                // Debug formatting ("Malformed", "SelfReference", ...).
                let reason_str = match reason {
                    SkipReason::Malformed => "malformed",
                    SkipReason::SelfReference => "self_reference",
                    SkipReason::UnknownHandle => "unknown_handle",
                    SkipReason::Cycle => "cycle",
                };
                skipped.push(serde_json::json!({
                    "handle": &span.handle,
                    "reason": reason_str,
                }));
            }
        }
    }

    // 9. Decode parent skills (stored as JSON string in DB).
    let parent_skills: Option<Vec<String>> = parent
        .skills
        .as_deref()
        .and_then(|s| serde_json::from_str(s).ok());

    // 10. Construct MentionPlan and call store.create_mention_children.
    let plan = MentionPlan {
        parent_id: task_id.clone(),
        children,
        idempotency_key,
        workspace: parent.workspace.clone(),
        skills: parent_skills,
        tenant: parent.tenant.clone(),
        created_by: Some(profile_from_env()),
    };

    let result = store
        .create_mention_children(plan)
        .context("Failed to create mention children")?;

    // 11. Output.
    if json {
        // WR-08 (Phase 36.3.7.8 code review): include "status":"ok" so
        // downstream parsers can disambiguate success vs. rejection envelopes
        // by a single discriminator field instead of testing for the presence
        // of `reason` vs. `children_created`.
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "status": "ok",
                "task_id": task_id,
                "mentions_parsed": spans.len(),
                "children_created": result.children.iter().map(|c| serde_json::json!({
                    "child_id": &c.child_id,
                    "assignee": &c.assignee,
                    "handle": &c.handle,
                })).collect::<Vec<_>>(),
                "skipped": skipped,
            }))?
        );
    } else {
        println!(
            "Created {} mention children for {}",
            result.children.len(),
            task_id
        );
    }
    Ok(0)
}

// ---------------------------------------------------------------------------
// cmd_archive
// ---------------------------------------------------------------------------

pub async fn cmd_archive(ids: Vec<String>, _json: bool, board: Option<&str>) -> Result<i32> {
    let mut store = open_store_for_board(board)?;
    let mut any_err = false;
    for id in &ids {
        match store.archive_task(id) {
            Ok(()) => println!("{}: archived", id),
            Err(e) => {
                eprintln!("{}: error: {}", id, e);
                any_err = true;
            }
        }
    }
    Ok(if any_err { 1 } else { 0 })
}

// ---------------------------------------------------------------------------
// cmd_tail
// ---------------------------------------------------------------------------

pub async fn cmd_tail(id: String, board: Option<&str>) -> Result<i32> {
    use std::time::Duration;
    let store = open_store_for_board(board)?;
    let mut last_id: i64 = {
        let events = store.get_events(&id).context("Failed to load events")?;
        events.last().map(|e| e.id).unwrap_or(0)
    };

    println!("Tailing events for {} (Ctrl-C to stop)...", id);
    loop {
        tokio::time::sleep(Duration::from_secs(1)).await;
        let events = store.get_events(&id).unwrap_or_default();
        let new_events: Vec<_> = events.into_iter().filter(|e| e.id > last_id).collect();
        for e in &new_events {
            print!("{}", format_event_line(e));
        }
        if let Some(e) = new_events.last() {
            last_id = e.id;
        }
    }
}

// ---------------------------------------------------------------------------
// cmd_watch
// ---------------------------------------------------------------------------

pub async fn cmd_watch(
    assignee: Option<String>,
    _tenant: Option<String>,
    _kinds: Option<String>,
    interval: Option<u64>,
    board: Option<&str>,
) -> Result<i32> {
    use std::time::Duration;
    let interval_secs = interval.unwrap_or(2);

    println!("Watching board events (Ctrl-C to stop)...");
    let mut last_id: i64 = {
        let store = open_store_for_board(board)?;
        let tasks = store.list_tasks(ListFilters {
            assignee: assignee.clone(),
            ..Default::default()
        })?;
        let mut max_id: i64 = 0;
        for t in &tasks {
            let events = store.get_events(&t.id).unwrap_or_default();
            if let Some(e) = events.last() {
                max_id = max_id.max(e.id);
            }
        }
        max_id
    };

    loop {
        tokio::time::sleep(Duration::from_secs(interval_secs)).await;
        let store = open_store_for_board(board)?;
        let tasks = store.list_tasks(ListFilters {
            assignee: assignee.clone(),
            ..Default::default()
        })?;
        for t in &tasks {
            let events = store.get_events(&t.id).unwrap_or_default();
            let new_events: Vec<_> = events.into_iter().filter(|e| e.id > last_id).collect();
            for e in &new_events {
                println!("[{}] {}", t.id, format_event_line(e).trim_end());
            }
            if let Some(e) = new_events.last() {
                last_id = e.id;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// cmd_runs
// ---------------------------------------------------------------------------

pub async fn cmd_runs(id: String, json: bool, board: Option<&str>) -> Result<i32> {
    let store = open_store_for_board(board)?;
    let runs = store.get_runs(&id).context("Failed to load runs")?;

    if json {
        println!("{}", serde_json::to_string_pretty(&runs)?);
    } else {
        print!("{}", format_runs_table(&runs));
    }
    Ok(0)
}

// ---------------------------------------------------------------------------
// cmd_assignees
// ---------------------------------------------------------------------------

pub async fn cmd_assignees(json: bool, board: Option<&str>) -> Result<i32> {
    let store = open_store_for_board(board)?;
    // Get all tasks and aggregate by assignee
    let tasks = store.list_tasks(ListFilters {
        archived: true, // include all
        ..Default::default()
    })?;

    let mut counts: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for t in &tasks {
        *counts.entry(t.assignee.clone()).or_insert(0) += 1;
    }

    let mut rows: Vec<(String, usize)> = counts.into_iter().collect();
    rows.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));

    if json {
        let val: Vec<_> = rows
            .iter()
            .map(|(a, c)| serde_json::json!({"assignee": a, "count": c}))
            .collect();
        println!("{}", serde_json::to_string_pretty(&val)?);
    } else {
        print!("{}", format_assignees(&rows));
    }
    Ok(0)
}

// ---------------------------------------------------------------------------
// cmd_dispatch
// ---------------------------------------------------------------------------

/// Phase 47.4 Plan 10 Task 2: `board: Option<&str>` (the parent `--board`
/// flag) is already ignored by `run_dispatch_tick`'s own all-boards sweep —
/// it always processes `"default"` plus every `ironhermes_kanban::paths::
/// list_boards()` slug, regardless of which single board this invocation's
/// `--board` flag or 4-tier resolver picked when opening `store` below. The
/// dispatch gate mirrors that REAL behavior: the store opened via
/// `open_store_for_board(board)` is swept as the `"default"` slot (exactly
/// as `run_dispatch_tick` reuses its own `ctx.store` for `"default"`), and
/// every OTHER named board is opened fresh and swept too. A per-board open
/// failure is skipped with a warning; the sweep continues (mirrors
/// `dispatcher.rs`'s own skip-on-error behavior). `run_dispatch_tick` itself
/// is not changed.
pub async fn cmd_dispatch(
    dry_run: bool,
    _max: Option<usize>,
    failure_limit: Option<usize>,
    json: bool,
    board: Option<&str>,
) -> Result<i32> {
    if dry_run {
        eprintln!("--dry-run not yet implemented for dispatch; proceeding with live dispatch");
    }

    let mut store = open_store_for_board(board).context("Failed to open kanban DB")?;

    // Phase 47.4 Plan 10 (GAP-1): hard pre-spawn dispatch gate. Refuse any
    // ready task whose assignee profile cannot resolve a provider key for
    // its configured main provider — BEFORE run_dispatch_tick ever attempts
    // to spawn it. Blocked tasks are written to status='blocked' with a
    // `dispatch gate: ` reason so the operator can audit/undo exactly the
    // tasks this gate blocked. Swept across every board `run_dispatch_tick`
    // will itself sweep (see doc comment above).
    let mut gate_blocked = super::dispatch_gate::refuse_undispatchable_ready_tasks(&mut store);
    let mut swept_boards = vec!["default".to_string()];
    match ironhermes_kanban::paths::list_boards() {
        Ok(named) => swept_boards.extend(named),
        Err(e) => {
            tracing::warn!(
                "[kanban] dispatch gate could not enumerate boards: {e}; using default only"
            );
        }
    }
    for slug in swept_boards.iter().filter(|s| s.as_str() != "default") {
        match KanbanStore::open_for_board(slug) {
            Ok(mut named_store) => {
                gate_blocked.extend(super::dispatch_gate::refuse_undispatchable_ready_tasks(
                    &mut named_store,
                ));
            }
            Err(e) => {
                tracing::warn!("[kanban] dispatch gate skipping board '{slug}': {e}");
            }
        }
    }

    let main_config = Config::load().unwrap_or_default();
    let mut config = load_kanban_config(&main_config);
    if let Some(fl) = failure_limit {
        config.failure_limit = fl as u32;
    }

    let store_arc = Arc::new(TokioMutex::new(store));
    let ctx = DispatcherContext::new(store_arc, config);

    run_dispatch_tick(&ctx)
        .await
        .context("Dispatch tick failed")?;

    if json {
        let gate_blocked_json: Vec<_> = gate_blocked
            .iter()
            .map(|(id, reason)| serde_json::json!({"task": id, "reason": reason}))
            .collect();
        println!(
            "{}",
            serde_json::json!({"status": "ok", "gate_blocked": gate_blocked_json})
        );
    } else {
        for (id, reason) in &gate_blocked {
            eprintln!("dispatch gate: task {id} blocked — {reason}");
        }
        println!("Dispatch tick completed.");
    }
    Ok(0)
}

// ---------------------------------------------------------------------------
// cmd_stats
// ---------------------------------------------------------------------------

pub async fn cmd_stats(json: bool, board: Option<&str>) -> Result<i32> {
    let store = open_store_for_board(board)?;
    let tasks = store.list_tasks(ListFilters {
        archived: true,
        ..Default::default()
    })?;

    let mut counts: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for t in &tasks {
        *counts.entry(t.status.clone()).or_insert(0) += 1;
    }

    let mut rows: Vec<(String, usize)> = counts.into_iter().collect();
    rows.sort_by_key(|(s, _)| s.clone());

    if json {
        let val: Vec<_> = rows
            .iter()
            .map(|(s, c)| serde_json::json!({"status": s, "count": c}))
            .collect();
        println!("{}", serde_json::to_string_pretty(&val)?);
    } else {
        print!("{}", format_stats(&rows));
    }
    Ok(0)
}

// ---------------------------------------------------------------------------
// cmd_log
// ---------------------------------------------------------------------------

pub async fn cmd_log(id: String, tail_bytes: Option<u64>, board: Option<&str>) -> Result<i32> {
    // Resolve the task's assignee so we read from the profile-scoped log
    // location (`~/.ironhermes/profiles/<assignee>/logs/kanban/`) written by
    // the worker spawn. Fall back to the legacy root path per-file below so
    // logs captured before the profile-scoped layout still display.
    let assignee = open_store_for_board(board)
        .ok()
        .and_then(|store| store.get_task(&id).ok())
        .map(|task| task.assignee)
        .unwrap_or_default();

    let resolve = |profile_scoped: std::path::PathBuf, legacy: std::path::PathBuf| {
        if profile_scoped.exists() || !legacy.exists() {
            profile_scoped
        } else {
            legacy
        }
    };
    let stdout_path = resolve(
        ironhermes_kanban::kanban_log_stdout_for(&assignee, &id),
        ironhermes_kanban::kanban_log_stdout(&id),
    );
    let stderr_path = resolve(
        ironhermes_kanban::kanban_log_stderr_for(&assignee, &id),
        ironhermes_kanban::kanban_log_stderr(&id),
    );

    for (label, path) in &[("STDOUT", &stdout_path), ("STDERR", &stderr_path)] {
        if path.exists() {
            println!("--- {} ---", label);
            let content = std::fs::read(path)
                .with_context(|| format!("Failed to read {}", path.display()))?;
            let slice = if let Some(n) = tail_bytes {
                let n = n as usize;
                if content.len() > n {
                    &content[content.len() - n..]
                } else {
                    &content
                }
            } else {
                &content
            };
            print!("{}", String::from_utf8_lossy(slice));
        } else {
            println!("--- {} (no log file) ---", label);
        }
    }
    Ok(0)
}

// ---------------------------------------------------------------------------
// cmd_context
// ---------------------------------------------------------------------------

pub async fn cmd_context(id: String, board: Option<&str>) -> Result<i32> {
    let store = open_store_for_board(board)?;
    let task = store.get_task(&id).context("Task not found")?;
    let runs = store.get_runs(&id).context("Failed to load runs")?;
    let events = store.get_events(&id).context("Failed to load events")?;
    let comments = get_comments(&store, &id).unwrap_or_default();

    let ctx = serde_json::json!({
        "task_id": task.id,
        "title": task.title,
        "body": task.body,
        "assignee": task.assignee,
        "status": task.status,
        "workspace": task.workspace,
        "tenant": task.tenant,
        "prior_runs": runs.iter().map(|r| serde_json::json!({
            "id": r.id,
            "outcome": r.outcome,
            "summary": r.summary,
            "error": r.error,
        })).collect::<Vec<_>>(),
        "comments": comments.iter().map(|c| serde_json::json!({
            "author": c.author,
            "body": c.body,
        })).collect::<Vec<_>>(),
        "events_tail": events.iter().rev().take(10).collect::<Vec<_>>().into_iter().rev().map(|e| serde_json::json!({
            "kind": e.kind,
            "payload": e.payload,
        })).collect::<Vec<_>>(),
    });

    println!("{}", serde_json::to_string_pretty(&ctx)?);
    Ok(0)
}

// ---------------------------------------------------------------------------
// cmd_gc
// ---------------------------------------------------------------------------

pub async fn cmd_gc(
    event_retention_days: Option<u64>,
    log_retention_days: Option<u64>,
    board: Option<&str>,
) -> Result<i32> {
    let store = open_store_for_board(board)?;
    let event_days = event_retention_days.unwrap_or(30);
    let log_days = log_retention_days.unwrap_or(30);

    // Delete old task_events (older than event_retention_days)
    let cutoff = {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs_f64();
        now - (event_days as f64 * 86400.0)
    };

    let deleted = store
        .conn
        .execute(
            "DELETE FROM task_events WHERE created_at < ?1",
            rusqlite::params![cutoff],
        )
        .context("Failed to delete old events")?;
    println!(
        "GC: deleted {} old events (older than {} days)",
        deleted, event_days
    );

    // Remove old log files. Worker logs now live under each assignee's
    // profile (`~/.ironhermes/profiles/<p>/logs/kanban/`), so sweep every
    // profile's log dir as well as the legacy root (`~/.ironhermes/logs/kanban/`)
    // for logs written before the profile-scoped layout.
    let log_cutoff =
        std::time::SystemTime::now() - std::time::Duration::from_secs(log_days * 86400);
    let sweep = |dir: &std::path::Path| -> usize {
        if !dir.exists() {
            return 0;
        }
        let Ok(read_dir) = std::fs::read_dir(dir) else {
            return 0;
        };
        let mut removed = 0usize;
        for entry in read_dir.flatten() {
            if let Ok(meta) = entry.metadata()
                && let Ok(modified) = meta.modified()
                && modified < log_cutoff
                && std::fs::remove_file(entry.path()).is_ok()
            {
                removed += 1;
            }
        }
        removed
    };

    // Collect every log dir to sweep: legacy root + one per profile.
    let mut log_dirs = vec![ironhermes_kanban::kanban_logs_dir()];
    let profiles_root = ironhermes_core::get_hermes_home().join(ironhermes_core::PROFILES_SUBDIR);
    if let Ok(read_dir) = std::fs::read_dir(&profiles_root) {
        for entry in read_dir.flatten() {
            if entry.path().is_dir()
                && let Some(name) = entry.file_name().to_str()
            {
                log_dirs.push(ironhermes_kanban::kanban_logs_dir_for_profile(name));
            }
        }
    }

    let removed: usize = log_dirs.iter().map(|d| sweep(d)).sum();
    println!(
        "GC: removed {} old log files (older than {} days)",
        removed, log_days
    );

    Ok(0)
}

// ---------------------------------------------------------------------------
// cmd_reclaim
// ---------------------------------------------------------------------------

pub async fn cmd_reclaim(id: String, board: Option<&str>) -> Result<i32> {
    let mut store = open_store_for_board(board)?;
    let task = store.get_task(&id).context("Task not found")?;

    if task.status != "running" {
        eprintln!("{} is not running (status: {})", id, task.status);
        return Ok(1);
    }

    let lock = task.claim_lock.as_deref().unwrap_or("");
    let rows = release_claim(&mut store.conn, &id, lock, "operator_reclaim")
        .context("Failed to release claim")?;

    if rows == 0 {
        eprintln!("{}: lock mismatch — claim may have been superseded", id);
        return Ok(1);
    }
    println!("{}: claim released (operator_reclaim)", id);
    Ok(0)
}

// ---------------------------------------------------------------------------
// cmd_reassign
// ---------------------------------------------------------------------------

pub async fn cmd_reassign(
    id: String,
    new_profile: String,
    reclaim: bool,
    board: Option<&str>,
) -> Result<i32> {
    let mut store = open_store_for_board(board)?;

    if reclaim {
        let task = store.get_task(&id).context("Task not found")?;
        if task.status == "running" {
            let lock = task.claim_lock.as_deref().unwrap_or("");
            release_claim(&mut store.conn, &id, lock, "operator_reclaim")
                .context("Failed to release claim before reassign")?;
            // rows returned is usize; we continue regardless (operator intent wins)
        }
    }

    store
        .assign_task(&id, &new_profile)
        .context("Failed to reassign task")?;
    println!("{}: reassigned to '{}'", id, new_profile);
    Ok(0)
}

// ---------------------------------------------------------------------------
// cmd_diagnostics
// ---------------------------------------------------------------------------

pub async fn cmd_diagnostics(json: bool, board: Option<&str>) -> Result<i32> {
    let store = open_store_for_board(board)?;
    let config = KanbanConfig::default();
    let threshold = config.stranded_threshold_seconds;

    let reports: Vec<StrandedReport> =
        diagnose_stranded(&store, threshold).context("Failed to run diagnostics")?;

    if json {
        let val: Vec<_> = reports
            .iter()
            .map(|r| {
                serde_json::json!({
                    "task_id": r.task_id,
                    "assignee": r.assignee,
                    "age_seconds": r.age_seconds,
                    "severity": format!("{:?}", r.severity),
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&val)?);
    } else {
        print!("{}", format_diagnostics(&reports));
    }
    Ok(0)
}

// ---------------------------------------------------------------------------
// cmd_daemon (deprecated — D-09)
// ---------------------------------------------------------------------------

pub async fn cmd_daemon(
    failure_limit: Option<usize>,
    pidfile: Option<String>,
    board: Option<&str>,
) -> Result<i32> {
    use ironhermes_kanban::run_dispatch_loop;
    use tokio_util::sync::CancellationToken;

    eprintln!(
        "WARNING: `kanban daemon --force` is deprecated (D-09). \
         The dispatcher is embedded in ironhermes-gateway by default. \
         Running both simultaneously is not supported."
    );

    let store = open_store_for_board(board).context("Failed to open kanban DB")?;
    let main_config = Config::load().unwrap_or_default();
    let mut config = load_kanban_config(&main_config);
    if let Some(fl) = failure_limit {
        config.failure_limit = fl as u32;
    }

    // Write pidfile if requested
    if let Some(ref pidfile_path) = pidfile {
        std::fs::write(pidfile_path, format!("{}", std::process::id()))
            .with_context(|| format!("Failed to write pidfile {}", pidfile_path))?;
    }

    let store_arc = Arc::new(TokioMutex::new(store));
    let ctx = Arc::new(DispatcherContext::new(store_arc, config));
    let cancel = CancellationToken::new();
    let cancel_for_signal = cancel.clone();

    // Spawn Ctrl-C watcher
    tokio::spawn(async move {
        let _ = tokio::signal::ctrl_c().await;
        cancel_for_signal.cancel();
    });

    println!("Kanban daemon running. Press Ctrl-C to stop.");
    run_dispatch_loop(ctx, cancel).await;
    println!("Kanban daemon stopped.");

    // Remove pidfile on exit
    if let Some(ref pidfile_path) = pidfile {
        let _ = std::fs::remove_file(pidfile_path);
    }

    Ok(0)
}

// ---------------------------------------------------------------------------
// Notify subscriptions (Phase 36.3.7.5 BUG-36.3.7.5-05)
// ---------------------------------------------------------------------------

/// `hermes kanban notify-subscribe <task-id> --platform P --chat-id C [--thread-id T]`
///
/// Records an EXPLICIT subscription (source='explicit') for the originating
/// chat. Auto-subscriptions from /kanban create use source='auto'.
pub async fn cmd_notify_subscribe(
    task_id: String,
    platform: String,
    chat_id: String,
    thread_id: Option<String>,
    board: Option<&str>,
) -> anyhow::Result<i32> {
    let mut store = match open_store_for_board(board) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: cannot open kanban DB: {}", e);
            return Ok(1);
        }
    };
    match store.append_subscription(
        &task_id,
        &platform,
        &chat_id,
        thread_id.as_deref(),
        "explicit",
    ) {
        Ok(id) => {
            println!(
                "Subscribed chat {} ({}) to task {} (subscription id {})",
                chat_id, platform, task_id, id
            );
            Ok(0)
        }
        Err(e) => {
            eprintln!("error: notify-subscribe: {}", e);
            Ok(1)
        }
    }
}

/// `hermes kanban notify-list [<task-id>] [--json]`
///
/// Lists all subscriptions, or filters by task_id. `--json` emits a serde
/// JSON array (Subscription's derive(Serialize)).
pub async fn cmd_notify_list(
    task_id: Option<String>,
    json: bool,
    board: Option<&str>,
) -> anyhow::Result<i32> {
    let store = match open_store_for_board(board) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: cannot open kanban DB: {}", e);
            return Ok(1);
        }
    };
    let subs = match &task_id {
        Some(t) => store.list_subscriptions_for_task(t),
        None => store.list_all_subscriptions(),
    };
    let subs = match subs {
        Ok(v) => v,
        Err(e) => {
            eprintln!("error: notify-list: {}", e);
            return Ok(1);
        }
    };
    if json {
        match serde_json::to_string(&subs) {
            Ok(s) => println!("{}", s),
            Err(e) => {
                eprintln!("error: serialize: {}", e);
                return Ok(1);
            }
        }
    } else if subs.is_empty() {
        println!("No subscriptions found.");
    } else {
        println!(
            "{:<6} {:<14} {:<10} {:<16} {:<10} {:<10}",
            "ID", "TASK", "PLATFORM", "CHAT_ID", "THREAD", "SOURCE"
        );
        for s in &subs {
            println!(
                "{:<6} {:<14} {:<10} {:<16} {:<10} {:<10}",
                s.id,
                s.task_id,
                s.platform,
                s.chat_id,
                if s.thread_id.is_empty() {
                    "-"
                } else {
                    s.thread_id.as_str()
                },
                s.source
            );
        }
    }
    Ok(0)
}

// ---------------------------------------------------------------------------
// Phase 36.3.7.10 — hermes kanban decompose / specify CLI verbs
// ---------------------------------------------------------------------------

/// Load the operator's KanbanConfig from the main config.yaml's `kanban:` block.
///
/// Fails gracefully: if `config.yaml` is missing or the `kanban:` block is absent,
/// returns `KanbanConfig::default()` (all defaults). This mirrors the gateway runner's
/// behavior.
///
/// `pub` (was `pub(crate)`) so the worker-mode dispatcher in `main.rs` can call
/// it alongside `build_runtime_judge_fn` when constructing the goal-loop wrapper
/// inputs (Phase 36.3.7.12-04 Plan 04 Task 2).
pub fn load_kanban_config(main_config: &Config) -> KanbanConfig {
    serde_yaml::from_value(main_config.kanban.clone()).unwrap_or_default()
}

/// Build the production DecomposeFn from operator config.
///
/// Three-tier cascade (RESEARCH §Q1):
///   Tier 1: `kanban_config.decomposer_model` — kanban-local override (non-empty wins).
///   Tier 2: `main_config.model.roles["kanban_decomposer"].model` — per-role override.
///   Tier 3: `main_config.model.default` — main provider's default model.
///
/// The resulting closure builds a system prompt encoding the canonical Markdown
/// body schema (RESEARCH §Q8), builds a user message with the task's title + body,
/// calls the provider's text-completion endpoint via `LlmClient::chat_completion`,
/// and parses the response as JSON into `DecomposeOutput`.
///
/// Returns `Err` if no provider can be constructed (e.g., no API key set), with
/// a clear message naming the config keys the operator must set.
fn build_runtime_decompose_fn(
    kanban_config: &KanbanConfig,
    main_config: &Config,
) -> anyhow::Result<DecomposeFn> {
    use ironhermes_agent::client::LlmClient;
    use ironhermes_core::ChatMessage;
    use ironhermes_core::provider::ProviderResolver;

    // Tier 1: kanban-local model override.
    // Tier 2: model.roles["kanban_decomposer"].model.
    // Tier 3: main provider's default model.
    let registry = ProviderResolver::build(main_config)?;

    // Resolve endpoint + model per the three-tier cascade.
    let (endpoint, resolved_model): (_, String) = if !kanban_config.decomposer_model.is_empty() {
        // Tier 1: kanban.decomposer_model is set — use main provider with this model.
        let ep = registry.resolve_for_main().clone();
        let model = kanban_config.decomposer_model.clone();
        (ep, model)
    } else if let Some(ep) = registry.resolve_role("kanban_decomposer") {
        // Tier 2: model.roles["kanban_decomposer"] resolves (carries endpoint + model).
        let model = ep.default_model.clone();
        (ep, model)
    } else {
        // Tier 3: fall back to main provider's default model.
        let ep = registry.resolve_for_main().clone();
        let model = ep.default_model.clone();
        (ep, model)
    };

    // Verify we have an API key (empty key → reject clearly).
    // CR-03 fix: fail closed regardless of scheme — the previous protocol
    // filter (`base_url.starts_with("https://")`) allowed misconfigured HTTP
    // endpoints to pass clearance, after which the closure would build an
    // LlmClient with `api_key = ""` and either silently send triage prose
    // over plaintext (if the gateway accepts empty keys) or surface a generic
    // LLM auth error rather than the actionable "missing key" guard.
    let api_key = endpoint.api_key.clone().unwrap_or_default();
    if api_key.is_empty() {
        anyhow::bail!(
            "decomposer model not configured — set `kanban.decomposer_model` in \
             config.yaml OR `auxiliary.kanban_decomposer` OR ensure `model.default` \
             is set with a valid API key (endpoint: {})",
            endpoint.base_url
        );
    }

    let base_url = endpoint.base_url.clone();
    let model_for_closure: String = resolved_model;

    // Build the closure (captures owned data — no lifetime issues).
    let decompose_fn: DecomposeFn = Arc::new(move |req: DecomposeRequest| {
        let base_url = base_url.clone();
        let api_key = api_key.clone();
        let model = model_for_closure.clone();

        Box::pin(async move {
            let client = LlmClient::new(&base_url, &api_key, &model);

            // System prompt: instructs the LLM to produce JSON output per the
            // canonical Markdown body schema (RESEARCH §Q8).
            // T-36.3.7.10-05-01: system prompt is TRUSTED (not user-supplied);
            // task title/body below is UNTRUSTED and treated as user message content.
            let system_prompt = "You are a kanban task decomposer. \
                Given a one-line triage task, produce a detailed task specification as JSON. \
                The JSON must have EXACTLY these fields:\n\
                {\n\
                  \"new_title\": \"<concise rewritten title (≤ 60 chars)>\",\n\
                  \"new_body\": \"<Markdown body with sections: ## Goal, ## Approach, ## Acceptance Criteria, ## Work Breakdown, ## Risk Register, ## Suggested Skills>\",\n\
                  \"children\": [\n\
                    {\"title\": \"<child task title>\", \"assignee\": \"\", \"body\": null}\n\
                  ]\n\
                }\n\
                `children` may be an empty array for the specify path.\n\
                Respond with ONLY the JSON object — no prose, no markdown fences.";

            let user_prompt = format!(
                "Task ID: {}\nTitle: {}\nBody: {}\nAssignee: {}",
                req.task_id,
                req.title,
                req.body.as_deref().unwrap_or("(none)"),
                req.assignee,
            );

            let messages = vec![
                ChatMessage::system(system_prompt),
                ChatMessage::user(&user_prompt),
            ];

            let response = client
                .chat_completion(&messages, None, None, Some(4096), Some(0.2), None)
                .await
                .map_err(|e| ironhermes_kanban::KanbanError::Other(anyhow::anyhow!("{}", e)))?;

            // Extract text content from the first choice.
            let raw_text = response
                .choices
                .first()
                .and_then(|c| c.message.content.as_ref())
                .and_then(|mc| mc.as_text())
                .unwrap_or("")
                .trim()
                .to_string();

            // Parse JSON response → DecomposeOutput.
            // On parse failure → Err so kernel appends DecomposeFailed event.
            // CR-01 fix: use char-boundary-aware truncate (raw_text.chars().take(N))
            // rather than a byte-slice — multi-byte UTF-8 (emoji, non-English LLM
            // output) would otherwise panic with `byte index N is not a char boundary`.
            let preview: String = raw_text.chars().take(200).collect();
            let parsed: serde_json::Value = serde_json::from_str(&raw_text).map_err(|e| {
                ironhermes_kanban::KanbanError::Other(anyhow::anyhow!(
                    "parse_error: LLM response is not valid JSON: {} (raw: {})",
                    e,
                    preview
                ))
            })?;

            // CR-02 fix: reject the LLM output when required fields are missing,
            // rather than silently substituting `(untitled)` / `""` defaults.
            // The latter behaviour would permanently overwrite the operator's
            // original triage body with the empty string AND promote
            // `triage → todo` in the same transaction — silent data loss.
            // Bubble up `Err(...)` so the kernel appends a `decompose_failed`
            // event and the task is retained in `triage`.
            let new_title = parsed["new_title"]
                .as_str()
                .ok_or_else(|| {
                    ironhermes_kanban::KanbanError::Other(anyhow::anyhow!(
                        "schema_error: LLM response missing required `new_title` field"
                    ))
                })?
                .to_string();
            let new_body = parsed["new_body"]
                .as_str()
                .ok_or_else(|| {
                    ironhermes_kanban::KanbanError::Other(anyhow::anyhow!(
                        "schema_error: LLM response missing required `new_body` field"
                    ))
                })?
                .to_string();

            let empty_vec = vec![];
            let children: Vec<ChildSpec> = parsed["children"]
                .as_array()
                .unwrap_or(&empty_vec)
                .iter()
                .map(|c| ChildSpec {
                    title: c["title"].as_str().unwrap_or("(child)").to_string(),
                    assignee: c["assignee"].as_str().unwrap_or("").to_string(),
                    body: c["body"].as_str().map(|s: &str| s.to_string()),
                })
                .collect();

            Ok(DecomposeOutput {
                new_title,
                new_body,
                children,
            })
        })
    });

    Ok(decompose_fn)
}

// ===========================================================================
// Phase 36.3.7.12 Plan 03 Task 2 — build_runtime_judge_fn (D-05)
// ===========================================================================
//
// Mirrors `build_runtime_decompose_fn` above one-for-one. Three-tier cascade:
//   Tier 1: `kanban_config.judge_model` — kanban-local override (non-empty wins).
//   Tier 2: `main_config.model.roles["kanban_judge"]` — per-role override.
//   Tier 3: `main_config.model.default` — main provider's default model.
//
// The hoisted helper `resolve_judge_model_and_endpoint` is the testable seat
// of the cascade: unit tests assert which model wins for each tier without
// executing the LLM closure. `build_runtime_judge_fn` wraps it and returns
// the runtime `JudgeFn` closure that Plan 04's loop wrapper will call.

/// Resolve the (endpoint, model) pair for the judge LLM call.
///
/// This is the cascade-only side-effect; lifted out of
/// `build_runtime_judge_fn` so unit tests can assert which tier won without
/// constructing or executing the closure.
///
/// Returns `Err` with an actionable message naming BOTH `kanban.judge_model`
/// AND `auxiliary.kanban_judge` when no provider can produce an API key —
/// mirrors the decomposer error pattern at commands.rs:1448-1454.
///
/// Phase 47.4 (D-14): `build_runtime_judge_fn` now calls the override-taking
/// sibling directly, so within the `ironhermes-cli` *binary* crate (which
/// declares its own `mod kanban;` in `main.rs`, compiling this file a second
/// time separately from the `ironhermes_cli` lib crate) this fn has no
/// internal caller and would otherwise trip `dead_code`. It stays `pub` and
/// unremoved because it is the required zero-arg-override entry point for
/// every existing external (lib) caller, including the crate's own test
/// suite — the `#[allow]` below documents the bin-vs-lib duplication, not a
/// real dead-code bug.
#[allow(dead_code)]
pub fn resolve_judge_model_and_endpoint(
    kanban_config: &KanbanConfig,
    main_config: &Config,
) -> anyhow::Result<(ironhermes_core::provider::ResolvedEndpoint, String)> {
    resolve_judge_model_and_endpoint_with_env_overrides(
        kanban_config,
        main_config,
        &std::collections::HashMap::new(),
    )
}

/// [`resolve_judge_model_and_endpoint`] with a profile-scoped API-key override
/// map (Phase 47.4 D-14).
///
/// The web server resolves a judge cascade for an arbitrary kanban worker
/// profile inside one running (multi-threaded) process, so the ordinary
/// `ProviderResolver::build` — which reads keys from the *server process*
/// environment (the ROOT profile) — would validate the operator's own key
/// instead of the profile being probed. This sibling threads `overrides`
/// (populated by the caller from the target profile's `.env`) through to
/// [`ironhermes_core::provider::ProviderResolver::build_with_env_overrides`]
/// so the cascade, the `with_context` message, and the fail-closed
/// empty-`api_key` bail all live in exactly one place — this function.
/// `resolve_judge_model_and_endpoint` delegates here with an empty map, so
/// its behavior for every existing caller is unchanged.
///
/// Phase 47.4 Plan 17 (CR-01): this is the operator's own interactive
/// resolution — `overrides` misses still fall back to the *process*
/// environment. For any "what would the SCRUBBED spawned worker see?"
/// question, use [`resolve_judge_model_and_endpoint_with_env_overrides_strict`]
/// instead; this function's behavior for every existing caller is unchanged.
pub fn resolve_judge_model_and_endpoint_with_env_overrides(
    kanban_config: &KanbanConfig,
    main_config: &Config,
    overrides: &std::collections::HashMap<String, String>,
) -> anyhow::Result<(ironhermes_core::provider::ResolvedEndpoint, String)> {
    resolve_judge_model_and_endpoint_with_env_scope(kanban_config, main_config, overrides, true)
}

/// [`resolve_judge_model_and_endpoint_with_env_overrides`] with the
/// process-environment fallback **disabled** (Phase 47.4 Plan 17, CR-01) —
/// `overrides` is the entire world, mirroring
/// [`ironhermes_core::provider::ProviderResolver::build_with_env_overrides_strict`].
///
/// Answers "what would the SCRUBBED spawned worker see?" — the question
/// VERIFY (`iron_hermes_ui`'s `build_probe_setup`) needs answered. A
/// judge-tier key present only in the *server process* environment (e.g. the
/// ROOT `.env` `main.rs` loads at startup) must never make this resolve `Ok`
/// for a DIFFERENT profile being probed.
pub fn resolve_judge_model_and_endpoint_with_env_overrides_strict(
    kanban_config: &KanbanConfig,
    main_config: &Config,
    overrides: &std::collections::HashMap<String, String>,
) -> anyhow::Result<(ironhermes_core::provider::ResolvedEndpoint, String)> {
    resolve_judge_model_and_endpoint_with_env_scope(kanban_config, main_config, overrides, false)
}

/// Shared body for [`resolve_judge_model_and_endpoint_with_env_overrides`]
/// and its `_strict` sibling (Phase 47.4 Plan 17, CR-01) — the three-tier
/// cascade, the `with_context` message, and the fail-closed empty-`api_key`
/// bail live in exactly this one place. `allow_process_env` selects between
/// [`ironhermes_core::provider::ProviderResolver::build_with_env_overrides`]
/// and its `_strict` sibling, mirroring `build_with_env_scope`'s own shape
/// (`ironhermes-core/src/provider.rs:244`).
fn resolve_judge_model_and_endpoint_with_env_scope(
    kanban_config: &KanbanConfig,
    main_config: &Config,
    overrides: &std::collections::HashMap<String, String>,
    allow_process_env: bool,
) -> anyhow::Result<(ironhermes_core::provider::ResolvedEndpoint, String)> {
    use ironhermes_core::models_cache::ModelsCache;
    use ironhermes_core::provider::ProviderResolver;

    let registry = if allow_process_env {
        ProviderResolver::build_with_env_overrides(main_config, ModelsCache::load(), overrides)
    } else {
        ProviderResolver::build_with_env_overrides_strict(main_config, ModelsCache::load(), overrides)
    }
    .with_context(|| {
        "judge model not configured — set `kanban.judge_model` in config.yaml \
     OR `auxiliary.kanban_judge` OR ensure `model.default` is set with a \
     valid provider"
    })?;

    // Resolve endpoint + model per the three-tier cascade.
    let (endpoint, resolved_model) = if !kanban_config.judge_model.is_empty() {
        // Tier 1: kanban.judge_model is set — use main provider with this model.
        let ep = registry.resolve_for_main().clone();
        let model = kanban_config.judge_model.clone();
        (ep, model)
    } else if let Some(ep) = registry.resolve_role("kanban_judge") {
        // Tier 2: model.roles["kanban_judge"] resolves (carries endpoint + model).
        let model = ep.default_model.clone();
        (ep, model)
    } else {
        // Tier 3: fall back to main provider's default model.
        let ep = registry.resolve_for_main().clone();
        let model = ep.default_model.clone();
        (ep, model)
    };

    // CR-03 mirror (commands.rs:1442-1454): fail closed when no API key is
    // resolvable — surface the actionable message rather than silently
    // calling chat_completion with an empty Bearer token.
    let api_key = endpoint.api_key.clone().unwrap_or_default();
    if api_key.is_empty() {
        anyhow::bail!(
            "judge model not configured — set `kanban.judge_model` in \
             config.yaml OR `auxiliary.kanban_judge` OR ensure `model.default` \
             is set with a valid API key (endpoint: {})",
            endpoint.base_url
        );
    }

    Ok((endpoint, resolved_model))
}

/// Build the production `JudgeFn` from operator config (D-05).
///
/// The resulting closure builds a static, trusted system prompt asking the
/// judge to return `{verdict: "met"|"not_met", reason: "..."}` JSON, sends
/// `title + body + worker_turn_output` as the user message, calls the
/// provider's text-completion endpoint via `LlmClient::chat_completion`,
/// and parses the response with fail-closed semantics:
///
/// - JSON parse failure → `Err` with raw response truncated to 200 chars
///   (T-36.3.7.12-03-T01 mitigation; mirrors commands.rs:1521-1527).
/// - Missing `verdict` field → `Err` with the same truncation.
/// - `verdict` value not in {"met","not_met"} (case-sensitive) → `Err`
///   (T-36.3.7.12-03-T02 mitigation).
///
/// Plan 04's goal-loop wrapper imports this through
/// `ironhermes_cli::kanban::commands::build_runtime_judge_fn`.
pub fn build_runtime_judge_fn(
    kanban_config: &KanbanConfig,
    main_config: &Config,
) -> anyhow::Result<ironhermes_kanban::JudgeFn> {
    build_runtime_judge_fn_with_env_overrides(
        kanban_config,
        main_config,
        &std::collections::HashMap::new(),
    )
}

/// [`build_runtime_judge_fn`] with a profile-scoped API-key override map
/// (Phase 47.4 D-14).
///
/// Builds the same `JudgeFn` closure — trusted static system prompt,
/// `LlmClient` call, and fail-closed JSON verdict parsing — against a
/// specific kanban worker profile's key material instead of the server
/// process's own environment. `build_runtime_judge_fn` delegates here with
/// an empty map. Stays in `ironhermes-cli` (not `ironhermes-kanban`) per the
/// locked crate-isolation fence in `ironhermes-kanban/src/judge.rs`.
///
/// Phase 47.4 Plan 17 (CR-01): this is the operator's own interactive
/// resolution — it resolves through
/// [`resolve_judge_model_and_endpoint_with_env_overrides`], which falls back
/// to the process environment. For any "what would the SCRUBBED spawned
/// worker see?" question, use
/// [`build_runtime_judge_fn_with_env_overrides_strict`] instead.
pub fn build_runtime_judge_fn_with_env_overrides(
    kanban_config: &KanbanConfig,
    main_config: &Config,
    overrides: &std::collections::HashMap<String, String>,
) -> anyhow::Result<ironhermes_kanban::JudgeFn> {
    build_runtime_judge_fn_with_env_scope(kanban_config, main_config, overrides, true)
}

/// [`build_runtime_judge_fn_with_env_overrides`] with the process-environment
/// fallback **disabled** (Phase 47.4 Plan 17, CR-01) — resolves through
/// [`resolve_judge_model_and_endpoint_with_env_overrides_strict`], so
/// `overrides` (the target profile's own `.env`) is the entire world.
///
/// Answers "what would the SCRUBBED spawned worker see?" — the question
/// VERIFY (`iron_hermes_ui`'s `build_probe_setup`) needs answered. A
/// judge-tier key present only in the *server process* environment must
/// never let this build a closure the spawned worker's scrubbed environment
/// (`.env_clear()` + profile `.env`) could not itself build.
///
/// Phase 47.4 (D-14 precedent, `resolve_judge_model_and_endpoint` above):
/// `ironhermes-cli` is a *binary* crate that declares its own `mod kanban;`
/// in `main.rs`, compiling this file a second time separately from the
/// `ironhermes_cli` *lib* crate. Its only caller is `iron_hermes_ui`'s
/// `build_probe_setup`, which depends on the lib crate — so within the bin
/// crate's own compilation this fn has no internal caller and would
/// otherwise trip `dead_code`. It stays `pub` and unremoved because it is
/// the required strict entry point for that external (lib) caller; the
/// `#[allow]` below documents the bin-vs-lib duplication, not a real
/// dead-code bug.
#[allow(dead_code)]
pub fn build_runtime_judge_fn_with_env_overrides_strict(
    kanban_config: &KanbanConfig,
    main_config: &Config,
    overrides: &std::collections::HashMap<String, String>,
) -> anyhow::Result<ironhermes_kanban::JudgeFn> {
    build_runtime_judge_fn_with_env_scope(kanban_config, main_config, overrides, false)
}

/// Shared body for [`build_runtime_judge_fn_with_env_overrides`] and its
/// `_strict` sibling (Phase 47.4 Plan 17, CR-01) — the `JudgeFn` closure
/// construction (trusted static system prompt, `LlmClient` call, fail-closed
/// JSON verdict parsing) lives in exactly this one place. `allow_process_env`
/// selects between [`resolve_judge_model_and_endpoint_with_env_overrides`]
/// and its `_strict` sibling for the underlying cascade resolution.
fn build_runtime_judge_fn_with_env_scope(
    kanban_config: &KanbanConfig,
    main_config: &Config,
    overrides: &std::collections::HashMap<String, String>,
    allow_process_env: bool,
) -> anyhow::Result<ironhermes_kanban::JudgeFn> {
    use ironhermes_agent::client::LlmClient;
    use ironhermes_core::ChatMessage;
    use ironhermes_kanban::{JudgeOutput, JudgeRequest, JudgeVerdict};

    let (endpoint, resolved_model) = if allow_process_env {
        resolve_judge_model_and_endpoint_with_env_overrides(kanban_config, main_config, overrides)?
    } else {
        resolve_judge_model_and_endpoint_with_env_overrides_strict(kanban_config, main_config, overrides)?
    };

    let base_url = endpoint.base_url.clone();
    let api_key = endpoint.api_key.clone().unwrap_or_default();
    let model_for_closure: String = resolved_model;

    let judge_fn: ironhermes_kanban::JudgeFn = Arc::new(move |req: JudgeRequest| {
        let base_url = base_url.clone();
        let api_key = api_key.clone();
        let model = model_for_closure.clone();

        Box::pin(async move {
            let client = LlmClient::new(&base_url, &api_key, &model);

            // System prompt: TRUSTED, static. The judge evaluates worker
            // output against `title + body` (= literal acceptance criteria,
            // CONTEXT.md D-01). Response shape is fixed JSON so Plan 04's
            // loop wrapper can dispatch on `verdict` without LLM-side prose
            // creep. T-36.3.7.12-03-T03: even on a successful prompt
            // injection in `body`, the response space is still constrained
            // to the JSON parser below.
            let system_prompt = "You evaluate whether worker output meets the acceptance criteria in \
                 title + body. Respond with JSON: \
                 {\"verdict\": \"met\" | \"not_met\", \"reason\": \"<short explanation>\"}.";

            let user_prompt = format!(
                "Title: {}\n\nAcceptance criteria (body):\n{}\n\nWorker output:\n{}",
                req.title, req.body, req.worker_turn_output,
            );

            let messages = vec![
                ChatMessage::system(system_prompt),
                ChatMessage::user(&user_prompt),
            ];

            let response = client
                .chat_completion(&messages, None, None, Some(1024), Some(0.0), None)
                .await
                .map_err(|e| ironhermes_kanban::KanbanError::Other(anyhow::anyhow!("{}", e)))?;

            // Extract text content from the first choice — same pattern as
            // build_runtime_decompose_fn at commands.rs:1507-1514.
            let raw_text = response
                .choices
                .first()
                .and_then(|c| c.message.content.as_ref())
                .and_then(|mc| mc.as_text())
                .unwrap_or("")
                .trim()
                .to_string();

            // 200-char char-boundary-safe preview (T-36.3.7.12-03-I01 V8
            // bounded-disclosure mitigation).
            let preview: String = raw_text.chars().take(200).collect();

            // Parse JSON — fail-closed on malformed body.
            let parsed: serde_json::Value = serde_json::from_str(&raw_text).map_err(|e| {
                ironhermes_kanban::KanbanError::Other(anyhow::anyhow!(
                    "judge response not JSON: {} (raw: {})",
                    e,
                    preview
                ))
            })?;

            // Required `verdict` string field — fail-closed on absence.
            let verdict_str = parsed
                .get("verdict")
                .and_then(|v| v.as_str())
                .ok_or_else(|| {
                    ironhermes_kanban::KanbanError::Other(anyhow::anyhow!(
                        "judge response missing 'verdict' field (raw: {})",
                        preview
                    ))
                })?;

            // Case-sensitive match against the two locked literals
            // (T-36.3.7.12-03-T02 mitigation).
            let verdict = match verdict_str {
                "met" => JudgeVerdict::Met,
                "not_met" => JudgeVerdict::NotMet,
                other => {
                    return Err(ironhermes_kanban::KanbanError::Other(anyhow::anyhow!(
                        "judge verdict not in {{met,not_met}}: {} (raw: {})",
                        other,
                        preview
                    )));
                }
            };

            // Reason field is optional; empty string when absent.
            let reason = parsed
                .get("reason")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();

            // turn is informational on the closure side — the wrapper that
            // emits judge_verdict events uses req.turn directly.
            let _ = req.turn;

            Ok(JudgeOutput { verdict, reason })
        })
    });

    Ok(judge_fn)
}

/// `hermes kanban decompose [<id>] [--all] [--tenant T] [--json] [--board <slug>]`
///
/// LLM-driven decomposer: fan a triage task into a child swarm.
/// Production DecomposeFn is constructed via `build_runtime_decompose_fn` using the
/// three-tier cascade (kanban.decomposer_model > model.roles.kanban_decomposer > main provider).
pub async fn cmd_decompose(
    id: Option<String>,
    all: bool,
    tenant: Option<String>,
    json: bool,
    board: Option<&str>,
) -> anyhow::Result<i32> {
    let store = open_store_for_board(board)?;
    // CR-WARNING-5 sourcing seam: KanbanStore::config() returns a KanbanConfig.
    // We also load the main config so we can do the three-tier model cascade.
    let _kanban_config_from_store = store.config();
    let main_config = Config::load().unwrap_or_default();
    let kanban_config = load_kanban_config(&main_config);

    let store_arc = Arc::new(TokioMutex::new(store));

    let decompose_fn = match build_runtime_decompose_fn(&kanban_config, &main_config) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("Decomposer not configured: {}", e);
            return Ok(1);
        }
    };

    if all {
        // T-36.3.7.10-05-03 (DoS / cost): cap at auto_decompose_per_tick.
        let cap = kanban_config.auto_decompose_per_tick as usize;
        let triage_tasks = {
            let store = store_arc.lock().await;
            store
                .list_tasks(ListFilters {
                    status: Some("triage".to_string()),
                    tenant: tenant.clone(),
                    ..Default::default()
                })
                .context("Failed to list triage tasks")?
        };

        let mut any_err = false;
        let mut output_rows: Vec<serde_json::Value> = Vec::new();

        for task in triage_tasks.into_iter().take(cap) {
            let task_id = task.id.clone();
            match decompose_triage_task(store_arc.clone(), &task_id, &decompose_fn, &kanban_config)
                .await
            {
                Ok(ids) => {
                    // WR-01 fix: re-read the task after success so the JSON
                    // envelope reports the LLM's rewritten title rather than
                    // the stale pre-decomposition `task.title` captured by
                    // `list_tasks` BEFORE the kernel rewrote it.
                    let new_title = {
                        let s = store_arc.lock().await;
                        s.get_task(&task_id).map(|t| t.title).unwrap_or_default()
                    };
                    if json {
                        output_rows.push(serde_json::json!({
                            "ok": true,
                            "task_id": task_id,
                            "reason": serde_json::Value::Null,
                            "fanout": ids.child_ids.len(),
                            "child_ids": ids.child_ids,
                            "new_title": new_title,
                        }));
                    } else {
                        println!(
                            "[{}] decomposed (children: {})",
                            task_id,
                            ids.child_ids.len()
                        );
                    }
                }
                Err(e) => {
                    eprintln!("[{}] decompose failed: {}", task_id, e);
                    any_err = true;
                    if json {
                        // On failure the task was retained in `triage`; the
                        // original title is still the correct value to surface.
                        output_rows.push(serde_json::json!({
                            "ok": false,
                            "task_id": task_id,
                            "reason": e.to_string(),
                            "fanout": 0,
                            "child_ids": [],
                            "new_title": task.title,
                        }));
                    }
                }
            }
        }

        if json {
            println!("{}", serde_json::to_string_pretty(&output_rows)?);
        }
        Ok(if any_err { 1 } else { 0 })
    } else {
        // Single-id mode: require explicit id.
        let task_id = id.ok_or_else(|| anyhow!("task id required when --all is not set"))?;

        // WR-02 fix: capture the original title BEFORE invoking the kernel
        // so the failure branch can report it; re-read after success so the
        // success branch reports the rewritten title.
        let original_title = {
            let s = store_arc.lock().await;
            s.get_task(&task_id).map(|t| t.title).unwrap_or_default()
        };
        match decompose_triage_task(store_arc.clone(), &task_id, &decompose_fn, &kanban_config)
            .await
        {
            Ok(ids) => {
                let new_title = {
                    let s = store_arc.lock().await;
                    s.get_task(&task_id).map(|t| t.title).unwrap_or_default()
                };
                if json {
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&serde_json::json!({
                            "ok": true,
                            "task_id": task_id,
                            "reason": serde_json::Value::Null,
                            "fanout": ids.child_ids.len(),
                            "child_ids": ids.child_ids,
                            "new_title": new_title,
                        }))?
                    );
                } else {
                    println!(
                        "[{}] decomposed (children: {})",
                        task_id,
                        ids.child_ids.len()
                    );
                }
                Ok(0)
            }
            Err(e) => {
                eprintln!("[{}] decompose failed: {}", task_id, e);
                if json {
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&serde_json::json!({
                            "ok": false,
                            "task_id": task_id,
                            "reason": e.to_string(),
                            "fanout": 0,
                            "child_ids": [],
                            "new_title": original_title,
                        }))?
                    );
                }
                Ok(1)
            }
        }
    }
}

/// `hermes kanban specify [<id>] [--all] [--tenant T] [--author NAME] [--json] [--board <slug>]`
///
/// LLM-driven specifier: rewrite triage task body + promote to todo.
/// Same shape as `cmd_decompose` but calls `specify_triage_task` (no fan-out).
/// The `--author` arg is threaded via the `author` field in the `HERMES_PROFILE`
/// environment variable override (passed to DecomposeRequest.config.orchestrator_profile
/// for author attribution in the `specified` event).
pub async fn cmd_specify(
    id: Option<String>,
    all: bool,
    tenant: Option<String>,
    author: Option<String>,
    json: bool,
    board: Option<&str>,
) -> anyhow::Result<i32> {
    let store = open_store_for_board(board)?;
    // CR-WARNING-5 sourcing seam: KanbanStore::config() returns a KanbanConfig.
    let _kanban_config_from_store = store.config();
    let main_config = Config::load().unwrap_or_default();
    let mut kanban_config = load_kanban_config(&main_config);

    // Thread author into orchestrator_profile if explicitly provided.
    if let Some(ref a) = author {
        kanban_config.orchestrator_profile = a.clone();
    } else if kanban_config.orchestrator_profile.is_empty() {
        kanban_config.orchestrator_profile = profile_from_env();
    }

    let store_arc = Arc::new(TokioMutex::new(store));

    let decompose_fn = match build_runtime_decompose_fn(&kanban_config, &main_config) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("Decomposer not configured: {}", e);
            return Ok(1);
        }
    };

    if all {
        // T-36.3.7.10-05-03: cap at auto_decompose_per_tick.
        let cap = kanban_config.auto_decompose_per_tick as usize;
        let triage_tasks = {
            let store = store_arc.lock().await;
            store
                .list_tasks(ListFilters {
                    status: Some("triage".to_string()),
                    tenant: tenant.clone(),
                    ..Default::default()
                })
                .context("Failed to list triage tasks")?
        };

        let mut any_err = false;
        let mut output_rows: Vec<serde_json::Value> = Vec::new();

        for task in triage_tasks.into_iter().take(cap) {
            let task_id = task.id.clone();
            match specify_triage_task(store_arc.clone(), &task_id, &decompose_fn, &kanban_config)
                .await
            {
                Ok(ids) => {
                    // WR-05 fix: surface root_promoted in the JSON envelope so
                    // operators can distinguish "this run promoted the task"
                    // from "the task was already specified — short-circuit".
                    // Re-read for the rewritten title too (WR-01/WR-02 sibling).
                    let new_title = {
                        let s = store_arc.lock().await;
                        s.get_task(&task_id).map(|t| t.title).unwrap_or_default()
                    };
                    if json {
                        output_rows.push(serde_json::json!({
                            "ok": true,
                            "task_id": task_id,
                            "reason": serde_json::Value::Null,
                            "new_title": new_title,
                            "root_promoted": ids.root_promoted,
                        }));
                    } else if ids.root_promoted {
                        println!("[{}] specified (promoted to todo)", task_id);
                    } else {
                        println!("[{}] (already specified — skipped)", task_id);
                    }
                }
                Err(e) => {
                    eprintln!("[{}] specify failed: {}", task_id, e);
                    any_err = true;
                    if json {
                        output_rows.push(serde_json::json!({
                            "ok": false,
                            "task_id": task_id,
                            "reason": e.to_string(),
                            "new_title": task.title,
                            "root_promoted": false,
                        }));
                    }
                }
            }
        }

        if json {
            println!("{}", serde_json::to_string_pretty(&output_rows)?);
        }
        Ok(if any_err { 1 } else { 0 })
    } else {
        let task_id = id.ok_or_else(|| anyhow!("task id required when --all is not set"))?;

        // WR-02 fix: capture the original title BEFORE invoking the kernel
        // so the failure branch can report it; re-read after success so the
        // success branch reports the rewritten title.
        let original_title = {
            let s = store_arc.lock().await;
            s.get_task(&task_id).map(|t| t.title).unwrap_or_default()
        };
        match specify_triage_task(store_arc.clone(), &task_id, &decompose_fn, &kanban_config).await
        {
            Ok(ids) => {
                let new_title = {
                    let s = store_arc.lock().await;
                    s.get_task(&task_id).map(|t| t.title).unwrap_or_default()
                };
                if json {
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&serde_json::json!({
                            "ok": true,
                            "task_id": task_id,
                            "reason": serde_json::Value::Null,
                            "new_title": new_title,
                            "root_promoted": ids.root_promoted,
                        }))?
                    );
                } else if ids.root_promoted {
                    println!("[{}] specified (promoted to todo)", task_id);
                } else {
                    println!("[{}] (already specified — skipped)", task_id);
                }
                Ok(0)
            }
            Err(e) => {
                eprintln!("[{}] specify failed: {}", task_id, e);
                if json {
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&serde_json::json!({
                            "ok": false,
                            "task_id": task_id,
                            "reason": e.to_string(),
                            "new_title": original_title,
                            "root_promoted": false,
                        }))?
                    );
                }
                Ok(1)
            }
        }
    }
}

/// `hermes kanban notify-unsubscribe <task-id> [--platform P] [--chat-id C]`
///
/// Removes subscription rows for a task. Without filters, removes ALL rows
/// for the task (operator emergency). With filters, scopes the delete.
pub async fn cmd_notify_unsubscribe(
    task_id: String,
    platform: Option<String>,
    chat_id: Option<String>,
    board: Option<&str>,
) -> anyhow::Result<i32> {
    let mut store = match open_store_for_board(board) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: cannot open kanban DB: {}", e);
            return Ok(1);
        }
    };
    match store.remove_subscriptions(&task_id, platform.as_deref(), chat_id.as_deref()) {
        Ok(n) => {
            println!("Removed {} subscription(s) for task {}.", n, task_id);
            Ok(0)
        }
        Err(e) => {
            eprintln!("error: notify-unsubscribe: {}", e);
            Ok(1)
        }
    }
}
