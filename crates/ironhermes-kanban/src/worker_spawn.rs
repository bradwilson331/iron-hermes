//! Worker subprocess spawn + env scrub (D-15 / D-16 / D-17 / D-18 / D-19 / D-28).
//!
//! This module owns two concerns:
//!
//! 1. **Env scrub** (`build_kanban_worker_env`) — builds the exact 7 safe
//!    system vars + up to 9 kanban env vars that the worker subprocess should
//!    receive. All other env vars from the dispatcher process are DROPPED.
//!    This is the security guarantee that prevents shell secrets
//!    (`OPENAI_API_KEY`, `ANTHROPIC_API_KEY`, etc.) from reaching the child
//!    process (T-36.3.7-03-01 / D-18 / INV-36.3.7-04).
//!
//! 2. **Subprocess spawn** (`spawn_worker`) — assembles the `ironhermes`
//!    command per D-15, calls `.env_clear()` before `.envs(...)`, redirects
//!    stdout/stderr to per-task log files (D-19), and returns the child PID.
//!
//! ## Env contract (D-17 / D-18)
//!
//! Each kanban/tenant var is emitted under BOTH the canonical `IRONHERMES_*` name
//! AND the legacy `HERMES_*` name during the deprecation window (D-COMPAT). The
//! `IRONHERMES_HOME` / `IRONHERMES_WORKER_BIN` / `SAFE_SYSTEM_VARS` / `env_clear`
//! behaviour is unchanged — the secret-scrub guarantee is preserved.
//!
//! ```text
//! SAFE_SYSTEM_VARS (pass-through when present in caller env):
//!   PATH, HOME, USER, LANG, TERM, RUST_LOG, IRONHERMES_HOME
//!
//! Kanban vars — each emitted twice: canonical (IRONHERMES_*) + legacy (HERMES_*):
//!   IRONHERMES_KANBAN_TASK / HERMES_KANBAN_TASK         = task.id
//!   IRONHERMES_KANBAN_DB   / HERMES_KANBAN_DB           = board_db_path_for_slug(board_slug)
//!   IRONHERMES_KANBAN_BOARD / HERMES_KANBAN_BOARD       = board_slug
//!   IRONHERMES_KANBAN_WORKSPACES_ROOT / HERMES_KANBAN_WORKSPACES_ROOT = kanban_workspaces_root()
//!   IRONHERMES_KANBAN_WORKSPACE / HERMES_KANBAN_WORKSPACE = workspace
//!   IRONHERMES_KANBAN_RUN_ID / HERMES_KANBAN_RUN_ID     = run.id
//!   IRONHERMES_KANBAN_CLAIM_LOCK / HERMES_KANBAN_CLAIM_LOCK = run.claim_lock
//!   HERMES_PROFILE                                      = task.assignee  (unchanged — out of scope)
//!   IRONHERMES_TENANT / HERMES_TENANT                   = task.tenant    (only when Some)
//!
//! Single IRONHERMES_* vars (no legacy twin):
//!   IRONHERMES_ARTIFACTS_DB                             = ironhermes_artifacts::default_db_path()  (root store redirect, D-01/D-04)
//!   IRONHERMES_ARTIFACTS_PROFILE                        = ironhermes_core::current_profile()        (operator profile bucket, Option A)
//! ```
//!
//! ## Spawn shape (D-15 / D-28)
//!
//! ```text
//! ironhermes --profile <assignee>
//!            chat -q "work kanban task <id>"
//! ```
//!
//! Static invariants checked by `tests/invariants_36_3_7.rs`:
//! - `build_kanban_worker_env` present (INV-36.3.7-02)
//! - `env_clear` present (INV-36.3.7-05)

use std::path::{Path, PathBuf};
use std::process::Stdio;

use tokio::process::Command;

use crate::error::{KanbanError, Result};
use crate::paths::{
    kanban_log_stderr_for, kanban_log_stdout_for, kanban_workspace_for, kanban_workspaces_root,
};
use crate::store::KanbanStore;
use crate::types::{Task, TaskRun};

// ---------------------------------------------------------------------------
// resolve_workspace_dir (D-01 / D-02)
// ---------------------------------------------------------------------------

/// Resolve a task's `workspace` string to a validated, canonical absolute
/// `PathBuf` — the single point where the worker's future CWD is decided
/// (46.4 Facet 1 / D-01 / D-02).
///
/// **Fail-loud guard (D-02):** returns `Err` when `workspace` is empty,
/// contains a literal `$` character (an unexpanded shell variable — the
/// exact symptom that created a stray `$IRONHERMES_KANBAN_WORKSPACE`
/// directory at repo root), or is not an absolute path. These three checks
/// run BEFORE any filesystem access.
///
/// **Scratch creation (D-31, preserved):** when the (already-absolute)
/// path lives under [`kanban_workspaces_root()`], the directory is created
/// eagerly via `create_dir_all` — this is the same eager-create behavior
/// `spawn_worker_for_board` has always had for scratch workspaces, just
/// relocated into this helper.
///
/// **Existence guard (D-02):** for non-scratch paths (`dir:`/`project:`
/// workspaces, whose absolute tail arrives here unprefixed — the caller in
/// `dispatcher.rs` strips the `dir:` prefix before invoking the spawn
/// path) the directory MUST already exist; a missing directory is a fail-
/// loud `Err`, not a silent create.
///
/// On success the path is canonicalized (resolves `.`/`..`/symlinks) so
/// `.current_dir(&resolved)` always receives a real, unambiguous directory.
///
/// Each error branch carries a distinct, specific message so the
/// dispatcher's `outcome='spawn_failed'` error string is actionable.
fn resolve_workspace_dir(workspace: &str) -> Result<PathBuf> {
    if workspace.is_empty() {
        return Err(KanbanError::Other(anyhow::anyhow!(
            "workspace path unresolved or missing: {workspace} (empty workspace string)"
        )));
    }
    if workspace.contains('$') {
        return Err(KanbanError::Other(anyhow::anyhow!(
            "workspace path unresolved or missing: {workspace} (contains an unexpanded shell variable '$')"
        )));
    }
    let ws_path = std::path::Path::new(workspace);
    if !ws_path.is_absolute() {
        return Err(KanbanError::Other(anyhow::anyhow!(
            "workspace path unresolved or missing: {workspace} (not an absolute path)"
        )));
    }

    // Scratch workspaces (D-31): eagerly create the dir if it's under the
    // workspaces root. dir:/project: workspaces are NOT created here — they
    // must already exist (checked below).
    let ws_root = kanban_workspaces_root();
    if ws_path.starts_with(&ws_root) {
        std::fs::create_dir_all(ws_path).map_err(|e| {
            KanbanError::Other(anyhow::anyhow!("create kanban workspace {workspace}: {e}"))
        })?;
    }

    if !ws_path.is_dir() {
        return Err(KanbanError::Other(anyhow::anyhow!(
            "workspace path unresolved or missing: {workspace} (directory does not exist)"
        )));
    }

    std::fs::canonicalize(ws_path).map_err(|e| {
        KanbanError::Other(anyhow::anyhow!(
            "workspace path unresolved or missing: {workspace} (canonicalize failed: {e})"
        ))
    })
}

// ---------------------------------------------------------------------------
// reinit_scratch_workspace_if_retry (D-05, 46.5)
// ---------------------------------------------------------------------------

/// On a retry of a **scratch** workspace task, wipe the scratch dir before
/// respawn so a prior crashed run's leftovers can't fool the retry into
/// thinking the work is already done (46.5 D-05).
///
/// # Scope — scratch ONLY, never `dir:`/`project:` (T-46.5-10)
///
/// Acts ONLY when BOTH conditions hold:
/// 1. `task.workspace.is_none()` — the task uses the default scratch
///    workspace. A `dir:<abs>` or `project:<repo>` task always has
///    `task.workspace = Some(..)`, so this check alone excludes both
///    persistent-handoff-worktree kinds (46.4 D-07/D-09) by construction —
///    the dispatcher's caller-side `dir:`/`project:` arms never invoke this
///    function in the first place (defense-in-depth #1).
/// 2. `task.consecutive_failures > 0` — this is a retry, not the first
///    spawn. A first spawn (0 failures) must never wipe a workspace it just
///    created.
///
/// **Defense-in-depth #2:** even though the resolved path is always derived
/// from [`kanban_workspace_for`] (never attacker/task controlled), the
/// resolved path is re-asserted to be confined under
/// [`kanban_workspaces_root()`] before any removal. A path that somehow
/// fails that confinement check is left untouched (`Ok(())`, no-op) rather
/// than removed — refuse rather than risk wiping the wrong directory.
///
/// Only removes the directory if it exists; does NOT recreate it — the
/// existing eager-create step in [`resolve_workspace_dir`] (called
/// downstream at actual spawn time) composes with this to leave the
/// workspace empty-but-present by the time the worker `Command` is built.
pub fn reinit_scratch_workspace_if_retry(task: &Task) -> Result<()> {
    if task.workspace.is_some() {
        return Ok(());
    }
    if task.consecutive_failures <= 0 {
        return Ok(());
    }

    let ws_path = kanban_workspace_for(&task.id);
    let ws_root = kanban_workspaces_root();
    if !ws_path.starts_with(&ws_root) {
        // Defense-in-depth: refuse to touch anything outside the scratch
        // root rather than risk an unintended removal.
        return Ok(());
    }

    if ws_path.exists() {
        std::fs::remove_dir_all(&ws_path).map_err(|e| {
            KanbanError::Other(anyhow::anyhow!(
                "reinit scratch workspace {}: {e}",
                ws_path.display()
            ))
        })?;
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// copy_attachments_into (D-05)
// ---------------------------------------------------------------------------

/// Copy every attachment recorded for `task_id` into `workspace`, so every
/// workspace kind (scratch, `dir:`, `project:`) is self-contained and inputs
/// travel with the workspace regardless of kind (46.4 Facet 2 / D-05).
///
/// # DATABASE-driven, not filesystem-driven (UAT fix, D-05)
///
/// The on-disk attachment layout has changed shape more than once (flat
/// `<task_id>/a_<id>` files from the original implementation, then
/// per-attachment-id directories `<task_id>/<id>/<filename>`). A filesystem
/// walk that assumes one specific layout silently misses attachments stored
/// under any other layout — which is exactly the bug this fixes: worker
/// spawns produced EMPTY workspaces because old flat-layout attachments were
/// skipped entirely, and even a matching layout copied bytes under an
/// extension-less on-disk name (`a_<id>`) that the worker could never
/// recognise as an image.
///
/// Instead, this function opens the board's [`KanbanStore`] (the same way
/// the web upload/list handlers do, via
/// [`KanbanStore::open_from_env_or_board`]) and reads
/// [`KanbanStore::list_attachments`] — the authoritative source of each
/// attachment's `stored_path` (absolute on-disk path, whatever the layout)
/// and `filename` (the ORIGINAL client-supplied name). Every attachment is
/// copied into `workspace.join(<original filename>)`, i.e. under its real
/// name, regardless of how/where it happens to live on disk. If two
/// attachments happen to share an original filename, last-write-wins into
/// the workspace (rare — accepted).
///
/// Called from [`spawn_worker_for_board`] right after the workspace is
/// resolved (this module) or created (dispatcher-side `project:` worktree
/// creation), and BEFORE the worker `Command` is spawned — never from the
/// web/CLI upload handler (that only writes into the attachments store
/// itself; the copy-in step lives exclusively here, at spawn time).
///
/// Returns `Ok(0)` when the task has no attachments (the common case).
/// Returns the number of files actually copied on success. A `stored_path`
/// that no longer exists on disk (e.g. manually moved/removed) is skipped
/// rather than failing the whole spawn.
pub fn copy_attachments_into(workspace: &Path, task_id: &str, board_slug: &str) -> Result<usize> {
    let store = KanbanStore::open_from_env_or_board(Some(board_slug))?;
    let metas = store.list_attachments(task_id)?;
    if metas.is_empty() {
        return Ok(0);
    }

    let mut copied = 0usize;
    for meta in metas {
        let source = Path::new(&meta.stored_path);
        if !source.is_file() {
            // Attachment row exists but the file is gone from disk — skip
            // rather than fail the whole spawn.
            continue;
        }
        // Defense-in-depth: never trust a stored filename as a path, even
        // though add_attachment already rejects `..`/separators at write
        // time. `file_name()` strips any directory components.
        let Some(leaf) = Path::new(&meta.filename).file_name() else {
            continue;
        };
        let dest = workspace.join(leaf);
        std::fs::copy(source, &dest).map_err(|e| {
            KanbanError::Other(anyhow::anyhow!(
                "copy kanban attachment {} -> {}: {e}",
                source.display(),
                dest.display()
            ))
        })?;
        copied += 1;
    }

    Ok(copied)
}

// ---------------------------------------------------------------------------
// Safe system vars allowlist (D-18)
// ---------------------------------------------------------------------------

/// The 7 system env vars that are allowed to pass through to the worker
/// subprocess (D-18). Everything else is dropped by `env_clear()`.
///
/// Reuses the `build_safe_env()` allowlist concept from
/// `crates/ironhermes-exec/src/sandbox.rs` but adapted for the kanban worker
/// spawn (not the Python RPC sandbox).
pub const SAFE_SYSTEM_VARS: &[&str] = &[
    "PATH",
    "HOME",
    "USER",
    "LANG",
    "TERM",
    "RUST_LOG",
    "IRONHERMES_HOME",
    "IRONHERMES_WORKER_BIN", // Phase 36.3.7.13 D-02: forward-compat for recursive worker spawn
];

/// Phase 36.3.7.13 D-02: resolve the ironhermes worker binary.
///
/// Reads `IRONHERMES_WORKER_BIN` first (lets `cargo run` in a worktree pin
/// workers to the same binary without the `~/.local/bin/ironhermes` symlink
/// dance from Phase 36.3.7.12 UAT). Falls back to `"ironhermes"` (PATH
/// lookup — pre-36.3.7.13 behavior preserved bit-for-bit).
///
/// Callers: [`spawn_worker`] at the `Command::new` site (line 254 area).
/// The env var also rides the `SAFE_SYSTEM_VARS` allowlist above so that
/// recursive worker spawn (dispatcher → worker → sub-worker) carries the
/// override forward without additional wiring.
pub fn resolve_worker_bin() -> String {
    std::env::var("IRONHERMES_WORKER_BIN").unwrap_or_else(|_| "ironhermes".to_string())
}

// ---------------------------------------------------------------------------
// build_kanban_worker_env
// ---------------------------------------------------------------------------

/// Build the env map for a kanban worker subprocess (D-17 / D-18).
///
/// Returns a `Vec<(String, String)>` suitable for
/// `tokio::process::Command::envs(...)`. The caller MUST call `.env_clear()`
/// on the `Command` before calling `.envs(...)` to ensure no other env vars
/// slip through (D-18 / INV-36.3.7-05).
///
/// # What is included
///
/// - Up to 7 safe system vars from [`SAFE_SYSTEM_VARS`] (only when present
///   in the current process env).
/// - 8 always-present kanban vars.
/// - `IRONHERMES_TENANT` (+ legacy twin) only when `task.tenant` is `Some`.
/// - `IRONHERMES_KANBAN_TASK_SKILLS` (+ legacy twin) only when `task.skills` is `Some` and
///   decodes to a non-empty `Vec<String>` (forward-compatible carrier for
///   skill extras; replaces the dropped `--skills` argv path, BUG-36.3.7-01).
///
/// # What is excluded
///
/// Every other env var, including `OPENAI_API_KEY`, `ANTHROPIC_API_KEY`,
/// `GITHUB_TOKEN`, `*_SECRET`, `*_PASSWORD`, etc. (T-36.3.7-03-01).
pub fn build_kanban_worker_env(
    task: &Task,
    run: &TaskRun,
    workspace: &str,
    board_slug: &str,
) -> Vec<(String, String)> {
    let mut env: Vec<(String, String)> = Vec::new();

    // Safe system pass-through (D-18): only include if present in caller env.
    for var in SAFE_SYSTEM_VARS {
        if let Ok(v) = std::env::var(var) {
            env.push((var.to_string(), v));
        }
    }

    // Helper: push both the canonical IRONHERMES_* twin AND the legacy HERMES_* twin.
    //
    // The IRONHERMES_* name is derived by replacing the `HERMES_` prefix with
    // `IRONHERMES_` so the pair never drifts.  Both vars carry the same value.
    //
    // TODO(deprecation): drop the legacy HERMES_* twin after <window>
    let push_both = |env: &mut Vec<(String, String)>, legacy: &str, value: String| {
        let canonical = "IRONHERMES_".to_string() + &legacy["HERMES_".len()..];
        env.push((canonical, value.clone()));
        env.push((legacy.to_string(), value));
    };

    // Kanban vars (D-17) — each pushed twice: canonical IRONHERMES_* first, legacy HERMES_* second.
    // IRONHERMES_KANBAN_TASK / HERMES_KANBAN_TASK — gates the 6 LLM tools in plan 04.
    push_both(&mut env, "HERMES_KANBAN_TASK", task.id.clone());
    // IRONHERMES_KANBAN_DB / HERMES_KANBAN_DB — path to the board's kanban.db.
    push_both(
        &mut env,
        "HERMES_KANBAN_DB",
        crate::paths::board_db_path_for_slug(board_slug)
            .to_string_lossy()
            .into_owned(),
    );
    // IRONHERMES_ARTIFACTS_DB — redirect the worker's artifact store to the ROOT
    // store (Phase 46.6 gap-closure, D-01/D-04). The worker runs under the
    // assignee PROFILE home, so without this its `artifact` publishes land in
    // `profiles/<assignee>/artifacts.db` — invisible to the operator gallery,
    // which reads the ROOT store. Computed here at DISPATCHER time (root home,
    // override unset) so `default_db_path()` returns the canonical root
    // `artifacts.db`. NOT dual-set: this is a new IRONHERMES_* var with no legacy
    // HERMES_* twin. Mirrors the IRONHERMES_KANBAN_DB board redirect above.
    env.push((
        ironhermes_artifacts::ARTIFACTS_DB_ENV.to_string(),
        ironhermes_artifacts::default_db_path()
            .to_string_lossy()
            .into_owned(),
    ));
    // IRONHERMES_ARTIFACTS_PROFILE — stamp the worker's artifacts with the
    // OPERATOR profile (Phase 46.6 gap-closure, Option A) so unattended kanban
    // producers surface in the operator gallery instead of the worker's
    // assignee bucket. Computed here at DISPATCHER time (operator context) =
    // `current_profile()`; the store redirect above only aligns the DB FILE,
    // this aligns the profile COLUMN. Not dual-set (new IRONHERMES_* var).
    env.push((
        ironhermes_core::ARTIFACTS_PROFILE_ENV.to_string(),
        ironhermes_core::current_profile(),
    ));
    // IRONHERMES_KANBAN_BOARD / HERMES_KANBAN_BOARD — the resolved board slug (Phase 36.3.7.9).
    push_both(&mut env, "HERMES_KANBAN_BOARD", board_slug.to_string());
    // IRONHERMES_KANBAN_WORKSPACES_ROOT / HERMES_KANBAN_WORKSPACES_ROOT (D-31).
    push_both(
        &mut env,
        "HERMES_KANBAN_WORKSPACES_ROOT",
        kanban_workspaces_root().to_string_lossy().into_owned(),
    );
    // IRONHERMES_KANBAN_WORKSPACE / HERMES_KANBAN_WORKSPACE — the specific workspace for this task.
    push_both(&mut env, "HERMES_KANBAN_WORKSPACE", workspace.to_string());
    // IRONHERMES_KANBAN_RUN_ID / HERMES_KANBAN_RUN_ID — protocol-terminator gate (D-22).
    push_both(&mut env, "HERMES_KANBAN_RUN_ID", run.id.clone());
    // IRONHERMES_KANBAN_CLAIM_LOCK / HERMES_KANBAN_CLAIM_LOCK — D-41 worker write gating.
    push_both(&mut env, "HERMES_KANBAN_CLAIM_LOCK", run.claim_lock.clone());
    // HERMES_PROFILE — the assignee profile slug. NOT dual-set (D-SCOPE: out of migration scope).
    env.push(("HERMES_PROFILE".into(), task.assignee.clone()));
    // IRONHERMES_TENANT / HERMES_TENANT — only when task has a tenant (D-38).
    if let Some(ref t) = task.tenant {
        push_both(&mut env, "HERMES_TENANT", t.clone());
    }
    // IRONHERMES_KANBAN_TASK_SKILLS / HERMES_KANBAN_TASK_SKILLS — forward-compatible carrier
    // for task-level skill extras (D-28 / BUG-36.3.7-01). Emitted only when task.skills is
    // Some AND decodes to a non-empty Vec<String>. Receiver-side consumption
    // is out of scope for 36.3.7.0 (see 36.3.7.0-01-SKILLS-EXTRAS-AUDIT.md).
    if let Some(ref skills_json) = task.skills
        && let Ok(extras) = serde_json::from_str::<Vec<String>>(skills_json)
        && !extras.is_empty()
    {
        push_both(&mut env, "HERMES_KANBAN_TASK_SKILLS", skills_json.clone());
    }

    // Phase 36.3.7.12 (D-03 / D-06): goal-mode env pair.
    //
    // Emitted ONLY when the task opts into goal mode. When goal_mode=false
    // (every existing card) this block is a no-op and the 8-var env contract
    // (INV-36.3.7-07) is preserved bit-for-bit.
    //
    // The two new vars ride the per-task env list path downstream of
    // env_clear() + SAFE_SYSTEM_VARS allowlist (T-36.3.7.12-02-I01: they
    // carry only a feature flag and an integer budget — zero secret
    // material — so no new exfil vector exists).
    //
    // Defensive 0 → 20 coercion: a struct-literal caller that forgets to
    // set goal_max_turns lands `0`. We re-apply D-03's "20" default here so
    // the worker harness never sees a 0-budget signal that could mask as
    // "no budget enforcement." This mirrors the producer-level coercion
    // already in KanbanStore::create_task (Plan 01).
    if task.goal_mode {
        push_both(&mut env, "HERMES_KANBAN_GOAL_MODE", "1".into());
        let budget = if task.goal_max_turns == 0 {
            20
        } else {
            task.goal_max_turns
        };
        push_both(&mut env, "HERMES_KANBAN_GOAL_MAX_TURNS", budget.to_string());

        // Phase 36.3.7.13 F-03 / D-E1: emit IRONHERMES_KANBAN_GOAL_TOOLSET (+ legacy twin)
        // always when goal_mode=true. NULL in the DB (legacy v2 cards) coerces to
        // "restricted" HERE (the dispatcher is the single resolver per D-E2).
        // The worker side reads this env var verbatim and does NOT apply a second
        // default — absent env var under goal_mode=1 triggers a LOUD tracing::warn!
        // in filter_for_goal_mode_if_applicable (Task 3) to surface misconfig.
        let toolset_preset = task.goal_toolset.as_deref().unwrap_or("restricted");
        push_both(
            &mut env,
            "HERMES_KANBAN_GOAL_TOOLSET",
            toolset_preset.to_string(),
        );
    }

    env
}

// ---------------------------------------------------------------------------
// spawn_worker
// ---------------------------------------------------------------------------

/// Spawn a kanban worker subprocess for a task (single-board / back-compat version).
///
/// Always passes `"default"` as the board slug. Retained for callers that do
/// not yet carry a resolved board slug. Prefer [`spawn_worker_for_board`] in
/// new multi-board code (Phase 36.3.7.9).
pub async fn spawn_worker(task: &Task, run: &TaskRun, workspace: &str) -> Result<u32> {
    spawn_worker_for_board(task, run, workspace, "default").await
}

/// Spawn a kanban worker subprocess for a task, with an explicit board slug.
///
/// Builds the `ironhermes --profile <P> chat -q "work kanban task <id>"`
/// command (D-15; D-28 superseded by 36.3.7.0 BUG-01 — extras now carried
/// via IRONHERMES_KANBAN_TASK_SKILLS env), applies the env scrub via
/// `.env_clear()` + `.envs(build_kanban_worker_env(..., board_slug))` (D-18 /
/// INV-36.3.7-05), and redirects stdout/stderr to per-task log files (D-19).
///
/// Returns the spawned child PID on success.
///
/// # Workspace resolution + CWD (D-01 / D-02 / D-31)
///
/// `workspace` is resolved via [`resolve_workspace_dir`] to a validated,
/// canonical absolute path BEFORE the `Command` is built, and that resolved
/// path is passed to `.current_dir(...)` on the worker `Command` — so the
/// worker subprocess (and every non-shell file tool it uses) always sees a
/// real directory, never the dispatcher's own CWD or a literal `$VAR`
/// string. If `workspace` starts with the `kanban_workspaces_root()` prefix
/// (i.e. it's a scratch workspace), the directory is created before
/// spawning. For `dir:<abs>` workspaces the directory must already exist —
/// an unresolvable/missing/relative/`$`-containing workspace fails loud
/// with `Err` before `.spawn()` is ever called. `project:<repo>` worktree
/// creation is dispatcher-managed (`dispatcher.rs::create_project_worktree`,
/// D-06/D-07) and already resolved to a real absolute path by the time it
/// reaches here — never delegated to the worker itself. Immediately after
/// the workspace is resolved, [`copy_attachments_into`] reads every
/// attachment recorded for `task.id` from the board's store and copies it
/// into the workspace under its original filename (D-05), so every
/// workspace kind is self-contained regardless of how/where the attachment
/// happens to live on disk.
pub async fn spawn_worker_for_board(
    task: &Task,
    run: &TaskRun,
    workspace: &str,
    board_slug: &str,
) -> Result<u32> {
    // Profile-scoped worker logs (D-19): logs land under the assignee's
    // profile (`~/.ironhermes/profiles/<assignee>/logs/kanban/`) so every
    // artifact an agent produces — sessions, memories, state, and now its
    // per-task stdout/stderr capture — lives together under one profile
    // instead of the root `~/.ironhermes/logs/kanban/`. The dispatcher runs
    // unscoped, so it composes the profile path from the assignee slug.
    let stdout_log = kanban_log_stdout_for(&task.assignee, &task.id);
    let stderr_log = kanban_log_stderr_for(&task.assignee, &task.id);

    // Create log parent directory (D-19).
    if let Some(parent) = stdout_log.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            KanbanError::Other(anyhow::anyhow!(
                "create kanban log dir {}: {e}",
                parent.display()
            ))
        })?;
    }

    // Resolve the workspace to a validated, canonical absolute path (D-01 /
    // D-02). Fails loud — before any log file or Command is created — when
    // workspace is empty, relative, or contains a literal '$' (unexpanded
    // shell variable); creates the scratch dir eagerly when applicable.
    let resolved_workspace = resolve_workspace_dir(workspace)?;
    let resolved_workspace_env = resolved_workspace.to_string_lossy().into_owned();

    // Copy every attachment recorded for this task (read from the store,
    // not the filesystem — D-05) into the resolved workspace, right after
    // the workspace is known to exist, before any log file or Command is
    // created. Self-contained regardless of workspace kind (scratch / dir:
    // / project:).
    copy_attachments_into(&resolved_workspace, &task.id, board_slug)?;

    // Open log files.
    let stdout_file = std::fs::File::create(&stdout_log).map_err(|e| {
        KanbanError::Other(anyhow::anyhow!(
            "create stdout log {}: {e}",
            stdout_log.display()
        ))
    })?;
    let stderr_file = std::fs::File::create(&stderr_log).map_err(|e| {
        KanbanError::Other(anyhow::anyhow!(
            "create stderr log {}: {e}",
            stderr_log.display()
        ))
    })?;

    // Assemble command (D-15):
    //   ironhermes --profile <assignee> chat -q "work kanban task <id>"
    //
    // No --skills flag: kanban tools are registered env-gated via
    // IRONHERMES_KANBAN_TASK (Plan 05 / register_kanban_tools_if_applicable).
    // Skill extras ride IRONHERMES_KANBAN_TASK_SKILLS env var (BUG-36.3.7-01).
    //
    // CRITICAL: .env_clear() BEFORE .envs(...) ensures no inherited shell
    // secrets reach the worker process (D-18 / INV-36.3.7-05).
    // Phase 36.3.7.13 D-02: use resolve_worker_bin() so IRONHERMES_WORKER_BIN
    // can pin the subprocess to a specific binary (worktree cargo-run scenario).
    let child = Command::new(resolve_worker_bin())
        .arg("--profile")
        .arg(&task.assignee)
        .arg("chat")
        .arg("-q")
        .arg(format!("work kanban task {}", task.id))
        .current_dir(&resolved_workspace)
        .env_clear()
        .envs(build_kanban_worker_env(
            task,
            run,
            &resolved_workspace_env,
            board_slug,
        ))
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout_file))
        .stderr(Stdio::from(stderr_file))
        .spawn()
        .map_err(|e| {
            KanbanError::Other(anyhow::anyhow!(
                "spawn ironhermes worker for task {}: {e}",
                task.id
            ))
        })?;

    let pid = child.id().unwrap_or(0);
    tracing::info!(
        event = "spawned",
        task_id = %task.id,
        profile = %task.assignee,
        pid = pid,
        workspace = workspace,
    );
    Ok(pid)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Task, TaskRun};

    fn fake_task(id: &str, assignee: &str) -> Task {
        Task {
            id: id.to_string(),
            title: "test task".to_string(),
            body: None,
            assignee: assignee.to_string(),
            status: "ready".to_string(),
            priority: 0,
            tenant: None,
            workspace: None,
            skills: None,
            idempotency_key: None,
            claim_lock: None,
            claim_expires: None,
            current_run_id: None,
            consecutive_failures: 0,
            max_retries: None,
            max_runtime_seconds: None,
            scheduled_at: None,
            workflow_template_id: None,
            current_step_key: None,
            created_by: None,
            created_at: 1_700_000_000.0,
            started_at: None,
            ended_at: None,
            // Phase 36.3.7.12 — fake_task defaults: goal mode off, budget 20.
            goal_mode: false,
            goal_max_turns: 20,
            goal_turns_used: 0,
            // Phase 36.3.7.13 D-B1: no toolset preset by default.
            goal_toolset: None,
            // Phase 46.4 D-10: no output_path set by default.
            output_path: None,
        }
    }

    fn fake_run(id: &str, task_id: &str, claim_lock: &str) -> TaskRun {
        TaskRun {
            id: id.to_string(),
            task_id: task_id.to_string(),
            claim_lock: claim_lock.to_string(),
            claim_pid: None,
            started_at: 1_700_000_000.0,
            ended_at: None,
            outcome: None,
            summary: None,
            metadata: None,
            error: None,
            log_path: None,
        }
    }

    /// ENV scrub: a secret set in the test process must NOT appear in the
    /// env returned by build_kanban_worker_env (T-36.3.7-03-01 / D-18).
    #[test]
    fn build_kanban_worker_env_scrubs_secrets() {
        // Plant a sentinel secret in the test process env.
        // Safety: test-only; the test suite does not run in parallel
        // within a single process so this is safe.
        unsafe {
            std::env::set_var("OPENAI_API_KEY", "sk-leak-test");
            std::env::set_var("ANTHROPIC_API_KEY", "ant-leak-test");
            std::env::set_var("GITHUB_TOKEN", "ghp_leak-test");
            std::env::set_var("MY_SECRET_KEY", "super-secret");
            std::env::set_var("MY_PASSWORD", "hunter2");
        }

        let task = fake_task("t_abc123", "alice");
        let run = fake_run("r_run001", "t_abc123", "host:123:uuid");
        let env = build_kanban_worker_env(&task, &run, "/tmp/ws", "default");

        let keys: Vec<&str> = env.iter().map(|(k, _)| k.as_str()).collect();

        assert!(
            !keys.contains(&"OPENAI_API_KEY"),
            "OPENAI_API_KEY must NOT appear in worker env"
        );
        assert!(
            !keys.contains(&"ANTHROPIC_API_KEY"),
            "ANTHROPIC_API_KEY must NOT appear in worker env"
        );
        assert!(
            !keys.contains(&"GITHUB_TOKEN"),
            "GITHUB_TOKEN must NOT appear in worker env"
        );
        assert!(
            !keys.contains(&"MY_SECRET_KEY"),
            "MY_SECRET_KEY must NOT appear in worker env"
        );
        assert!(
            !keys.contains(&"MY_PASSWORD"),
            "MY_PASSWORD must NOT appear in worker env"
        );
    }

    /// 8 always-present kanban vars must be in the output (HERMES_TENANT is
    /// optional, so only 8 are guaranteed when task.tenant is None).
    /// Each in-scope var also asserts its IRONHERMES_* twin (D-COMPAT dual-set).
    #[test]
    fn build_kanban_worker_env_includes_eight_kanban_vars() {
        let task = fake_task("t_def456", "bob");
        let run = fake_run("r_run002", "t_def456", "host:456:uuid2");
        let env = build_kanban_worker_env(&task, &run, "/tmp/ws2", "default");

        let keys: Vec<&str> = env.iter().map(|(k, _)| k.as_str()).collect();

        // Legacy keys — must still be present (D-COMPAT: legacy twin always set).
        for required in &[
            "HERMES_KANBAN_TASK",
            "HERMES_KANBAN_DB",
            "HERMES_KANBAN_BOARD",
            "HERMES_KANBAN_WORKSPACES_ROOT",
            "HERMES_KANBAN_WORKSPACE",
            "HERMES_KANBAN_RUN_ID",
            "HERMES_KANBAN_CLAIM_LOCK",
            "HERMES_PROFILE",
        ] {
            assert!(
                keys.contains(required),
                "required kanban var {required} missing from worker env"
            );
        }
        // Canonical IRONHERMES_* twins — must also be present (D-COMPAT dual-set).
        for canonical in &[
            "IRONHERMES_KANBAN_TASK",
            "IRONHERMES_KANBAN_DB",
            "IRONHERMES_KANBAN_BOARD",
            "IRONHERMES_KANBAN_WORKSPACES_ROOT",
            "IRONHERMES_KANBAN_WORKSPACE",
            "IRONHERMES_KANBAN_RUN_ID",
            "IRONHERMES_KANBAN_CLAIM_LOCK",
        ] {
            assert!(
                keys.contains(canonical),
                "IRONHERMES_* twin {canonical} missing from worker env (D-COMPAT)"
            );
        }
    }

    /// HERMES_TENANT must only appear when task.tenant is Some.
    /// IRONHERMES_TENANT twin must also be present (D-COMPAT dual-set).
    #[test]
    fn build_kanban_worker_env_includes_tenant_when_set() {
        let mut task = fake_task("t_ghi789", "carol");
        task.tenant = Some("acme".to_string());

        let run = fake_run("r_run003", "t_ghi789", "host:789:uuid3");
        let env = build_kanban_worker_env(&task, &run, "/tmp/ws3", "default");

        let found_legacy = env.iter().find(|(k, _)| k == "HERMES_TENANT");
        assert!(
            found_legacy.is_some(),
            "HERMES_TENANT must be present when task.tenant is Some"
        );
        assert_eq!(found_legacy.unwrap().1, "acme");

        // D-COMPAT: IRONHERMES_TENANT twin must also carry the same value.
        let found_canonical = env.iter().find(|(k, _)| k == "IRONHERMES_TENANT");
        assert!(
            found_canonical.is_some(),
            "IRONHERMES_TENANT twin must be present when task.tenant is Some (D-COMPAT)"
        );
        assert_eq!(found_canonical.unwrap().1, "acme");
    }

    /// HERMES_KANBAN_TASK must equal task.id exactly.
    /// IRONHERMES_KANBAN_TASK twin must carry the same value (D-COMPAT).
    #[test]
    fn build_kanban_worker_env_task_id_matches() {
        let task = fake_task("t_specific_id", "diana");
        let run = fake_run("r_run004", "t_specific_id", "host:1:uuid4");
        let env = build_kanban_worker_env(&task, &run, "/tmp/ws4", "default");

        let task_val = env
            .iter()
            .find(|(k, _)| k == "HERMES_KANBAN_TASK")
            .map(|(_, v)| v.as_str());
        assert_eq!(task_val, Some("t_specific_id"));

        // D-COMPAT: IRONHERMES_KANBAN_TASK twin must match.
        let canonical_task_val = env
            .iter()
            .find(|(k, _)| k == "IRONHERMES_KANBAN_TASK")
            .map(|(_, v)| v.as_str());
        assert_eq!(canonical_task_val, Some("t_specific_id"));
    }

    /// HERMES_KANBAN_BOARD must equal "default" when board_slug is "default".
    #[test]
    fn build_kanban_worker_env_board_is_default() {
        let task = fake_task("t_board_test", "evan");
        let run = fake_run("r_run005", "t_board_test", "host:2:uuid5");
        let env = build_kanban_worker_env(&task, &run, "/tmp/ws5", "default");

        let board = env
            .iter()
            .find(|(k, _)| k == "HERMES_KANBAN_BOARD")
            .map(|(_, v)| v.as_str());
        assert_eq!(board, Some("default"));
    }

    // -----------------------------------------------------------------------
    // Phase 36.3.7.12 Plan 02 Task 2 — goal-mode env append
    // -----------------------------------------------------------------------

    /// D-06 / INV-36.3.7-07: when `task.goal_mode == false` the existing
    /// 8-var env contract is preserved — NO `IRONHERMES_KANBAN_GOAL_*` or
    /// `IRONHERMES_KANBAN_GOAL_*` keys appear in the returned env list.
    #[test]
    fn goal_mode_off_env_unchanged() {
        let task = fake_task("t_goal_off", "alice");
        // fake_task defaults: goal_mode: false, goal_max_turns: 20.
        let run = fake_run("r_goal_off", "t_goal_off", "host:1:goal_off");
        let env = build_kanban_worker_env(&task, &run, "/tmp/ws_goal_off", "default");

        let goal_keys: Vec<&str> = env
            .iter()
            .map(|(k, _)| k.as_str())
            .filter(|k| {
                k.starts_with("HERMES_KANBAN_GOAL_") || k.starts_with("IRONHERMES_KANBAN_GOAL_")
            })
            .collect();

        assert!(
            goal_keys.is_empty(),
            "goal_mode=false must NOT emit any HERMES_KANBAN_GOAL_* or IRONHERMES_KANBAN_GOAL_* keys; got {goal_keys:?}"
        );
    }

    /// D-03 / D-06: `task.goal_mode == true` with a non-default budget
    /// emits `IRONHERMES_KANBAN_GOAL_MODE=1`, `IRONHERMES_KANBAN_GOAL_MAX_TURNS=<budget>`,
    /// AND their `IRONHERMES_*` twins (D-COMPAT dual-set).
    #[test]
    fn goal_mode_on_env_has_budget() {
        let mut task = fake_task("t_goal_on", "bob");
        task.goal_mode = true;
        task.goal_max_turns = 7;
        let run = fake_run("r_goal_on", "t_goal_on", "host:2:goal_on");
        let env = build_kanban_worker_env(&task, &run, "/tmp/ws_goal_on", "default");

        let mode = env
            .iter()
            .find(|(k, _)| k == "HERMES_KANBAN_GOAL_MODE")
            .map(|(_, v)| v.as_str());
        assert_eq!(
            mode,
            Some("1"),
            "HERMES_KANBAN_GOAL_MODE must equal \"1\" when goal_mode=true"
        );

        let budget = env
            .iter()
            .find(|(k, _)| k == "HERMES_KANBAN_GOAL_MAX_TURNS")
            .map(|(_, v)| v.as_str());
        assert_eq!(
            budget,
            Some("7"),
            "HERMES_KANBAN_GOAL_MAX_TURNS must equal goal_max_turns.to_string()"
        );

        // D-COMPAT: IRONHERMES_* twins must also be present with identical values.
        let canonical_mode = env
            .iter()
            .find(|(k, _)| k == "IRONHERMES_KANBAN_GOAL_MODE")
            .map(|(_, v)| v.as_str());
        assert_eq!(
            canonical_mode,
            Some("1"),
            "IRONHERMES_KANBAN_GOAL_MODE twin must equal \"1\" (D-COMPAT)"
        );

        let canonical_budget = env
            .iter()
            .find(|(k, _)| k == "IRONHERMES_KANBAN_GOAL_MAX_TURNS")
            .map(|(_, v)| v.as_str());
        assert_eq!(
            canonical_budget,
            Some("7"),
            "IRONHERMES_KANBAN_GOAL_MAX_TURNS twin must equal goal_max_turns.to_string() (D-COMPAT)"
        );

        // Exactly one of each — guards against accidental double-push.
        let mode_count = env
            .iter()
            .filter(|(k, _)| k == "HERMES_KANBAN_GOAL_MODE")
            .count();
        let budget_count = env
            .iter()
            .filter(|(k, _)| k == "HERMES_KANBAN_GOAL_MAX_TURNS")
            .count();
        assert_eq!(
            mode_count, 1,
            "HERMES_KANBAN_GOAL_MODE must appear exactly once"
        );
        assert_eq!(
            budget_count, 1,
            "HERMES_KANBAN_GOAL_MAX_TURNS must appear exactly once"
        );
    }

    /// D-03 defensive coercion: `goal_mode == true` paired with a 0 budget
    /// (struct-literal default) must NOT emit "0" — the env value falls
    /// back to "20" (the D-03 documented default).
    #[test]
    fn goal_mode_on_with_zero_budget_falls_back_to_twenty() {
        let mut task = fake_task("t_goal_zero", "carol");
        task.goal_mode = true;
        task.goal_max_turns = 0; // defensive: caller forgot the budget
        let run = fake_run("r_goal_zero", "t_goal_zero", "host:3:goal_zero");
        let env = build_kanban_worker_env(&task, &run, "/tmp/ws_goal_zero", "default");

        let budget = env
            .iter()
            .find(|(k, _)| k == "HERMES_KANBAN_GOAL_MAX_TURNS")
            .map(|(_, v)| v.as_str());
        assert_eq!(
            budget,
            Some("20"),
            "0 budget must defensively coerce to \"20\" in the emitted env"
        );

        // D-COMPAT: IRONHERMES_KANBAN_GOAL_MAX_TURNS twin must also coerce to "20".
        let canonical_budget = env
            .iter()
            .find(|(k, _)| k == "IRONHERMES_KANBAN_GOAL_MAX_TURNS")
            .map(|(_, v)| v.as_str());
        assert_eq!(
            canonical_budget,
            Some("20"),
            "IRONHERMES_KANBAN_GOAL_MAX_TURNS twin must also coerce to \"20\" (D-COMPAT)"
        );
    }

    // -----------------------------------------------------------------------
    // Phase 36.3.7.13 Plan 03 Task 2 — IRONHERMES_KANBAN_GOAL_TOOLSET emission
    // -----------------------------------------------------------------------

    /// D-E1: goal_mode=true + goal_toolset=None (NULL in DB) must emit
    /// HERMES_KANBAN_GOAL_TOOLSET="restricted" (dispatcher-side NULL coercion),
    /// AND its IRONHERMES_* twin (D-COMPAT dual-set).
    #[test]
    fn goal_toolset_null_coerces_to_restricted() {
        let mut task = fake_task("t_toolset_null", "alice");
        task.goal_mode = true;
        task.goal_toolset = None; // NULL in DB
        let run = fake_run("r_ts_null", "t_toolset_null", "host:1:ts_null");
        let env = build_kanban_worker_env(&task, &run, "/tmp/ws_ts_null", "default");

        let found = env
            .iter()
            .find(|(k, _)| k == "HERMES_KANBAN_GOAL_TOOLSET")
            .map(|(_, v)| v.as_str());
        assert_eq!(
            found,
            Some("restricted"),
            "goal_toolset=None must coerce to \"restricted\" in emitted env (D-E1 dispatcher-side coercion)"
        );

        // D-COMPAT: IRONHERMES_KANBAN_GOAL_TOOLSET twin must match.
        let canonical = env
            .iter()
            .find(|(k, _)| k == "IRONHERMES_KANBAN_GOAL_TOOLSET")
            .map(|(_, v)| v.as_str());
        assert_eq!(
            canonical,
            Some("restricted"),
            "IRONHERMES_KANBAN_GOAL_TOOLSET twin must also coerce to \"restricted\" (D-COMPAT)"
        );
    }

    /// D-E1: goal_mode=true + goal_toolset=Some("extended") must emit
    /// HERMES_KANBAN_GOAL_TOOLSET="extended" verbatim (no override),
    /// AND its IRONHERMES_* twin (D-COMPAT dual-set).
    #[test]
    fn goal_toolset_some_passthrough() {
        let mut task = fake_task("t_toolset_ext", "bob");
        task.goal_mode = true;
        task.goal_toolset = Some("extended".to_string());
        let run = fake_run("r_ts_ext", "t_toolset_ext", "host:2:ts_ext");
        let env = build_kanban_worker_env(&task, &run, "/tmp/ws_ts_ext", "default");

        let found = env
            .iter()
            .find(|(k, _)| k == "HERMES_KANBAN_GOAL_TOOLSET")
            .map(|(_, v)| v.as_str());
        assert_eq!(
            found,
            Some("extended"),
            "goal_toolset=Some(\"extended\") must be emitted verbatim (D-E2 single-source-of-truth)"
        );

        // D-COMPAT: IRONHERMES_KANBAN_GOAL_TOOLSET twin must match.
        let canonical = env
            .iter()
            .find(|(k, _)| k == "IRONHERMES_KANBAN_GOAL_TOOLSET")
            .map(|(_, v)| v.as_str());
        assert_eq!(
            canonical,
            Some("extended"),
            "IRONHERMES_KANBAN_GOAL_TOOLSET twin must pass through verbatim (D-COMPAT)"
        );
    }

    /// D-E1 / D-06: goal_mode=false must NOT emit HERMES_KANBAN_GOAL_TOOLSET
    /// or IRONHERMES_KANBAN_GOAL_TOOLSET (env vars belong to goal-mode path only).
    #[test]
    fn goal_toolset_not_emitted_when_goal_mode_off() {
        let task = fake_task("t_toolset_off", "carol");
        // fake_task defaults: goal_mode: false, goal_toolset: None.
        let run = fake_run("r_ts_off", "t_toolset_off", "host:3:ts_off");
        let env = build_kanban_worker_env(&task, &run, "/tmp/ws_ts_off", "default");

        let found = env.iter().find(|(k, _)| k == "HERMES_KANBAN_GOAL_TOOLSET");
        assert!(
            found.is_none(),
            "HERMES_KANBAN_GOAL_TOOLSET must NOT appear in env when goal_mode=false; got: {found:?}"
        );
        let found_canonical = env
            .iter()
            .find(|(k, _)| k == "IRONHERMES_KANBAN_GOAL_TOOLSET");
        assert!(
            found_canonical.is_none(),
            "IRONHERMES_KANBAN_GOAL_TOOLSET must NOT appear in env when goal_mode=false; got: {found_canonical:?}"
        );
    }

    /// IRONHERMES_KANBAN_BOARD and IRONHERMES_KANBAN_DB must reflect the named slug
    /// when board_slug is a non-default value (Phase 36.3.7.9 D-04).
    #[test]
    fn build_kanban_worker_env_board_slug_propagates() {
        let task = fake_task("t_slug_test", "alice");
        let run = fake_run("r_run999", "t_slug_test", "host:2:uuid9");
        let env = build_kanban_worker_env(&task, &run, "/tmp/ws_slug", "atm10-server");

        let board = env
            .iter()
            .find(|(k, _)| k == "HERMES_KANBAN_BOARD")
            .map(|(_, v)| v.as_str());
        assert_eq!(board, Some("atm10-server"));

        let db = env
            .iter()
            .find(|(k, _)| k == "HERMES_KANBAN_DB")
            .map(|(_, v)| v.as_str())
            .unwrap_or("");
        assert!(
            db.contains("boards/atm10-server/kanban.db"),
            "HERMES_KANBAN_DB must contain 'boards/atm10-server/kanban.db', got: {db}"
        );

        // D-COMPAT: IRONHERMES_KANBAN_BOARD and IRONHERMES_KANBAN_DB twins must also be present.
        let canonical_board = env
            .iter()
            .find(|(k, _)| k == "IRONHERMES_KANBAN_BOARD")
            .map(|(_, v)| v.as_str());
        assert_eq!(
            canonical_board,
            Some("atm10-server"),
            "IRONHERMES_KANBAN_BOARD twin must match board slug (D-COMPAT)"
        );

        let canonical_db = env
            .iter()
            .find(|(k, _)| k == "IRONHERMES_KANBAN_DB")
            .map(|(_, v)| v.as_str())
            .unwrap_or("");
        assert!(
            canonical_db.contains("boards/atm10-server/kanban.db"),
            "IRONHERMES_KANBAN_DB twin must contain 'boards/atm10-server/kanban.db', got: {canonical_db}"
        );
    }

    // -----------------------------------------------------------------------
    // Phase 46.4 Plan 02 — resolve_workspace_dir (D-01 / D-02)
    // -----------------------------------------------------------------------

    /// RAII guard that sets an env var and restores the previous value on
    /// drop. Mirrors the identical helper in `paths.rs` tests — kept
    /// module-local rather than shared to avoid widening any pub surface.
    struct ScopedEnv {
        key: String,
        prev: Option<String>,
    }

    impl ScopedEnv {
        fn set(key: &str, value: &str) -> Self {
            let prev = std::env::var(key).ok();
            // Safety: test-only; the test suite does not run this module's
            // tests in parallel within a single process against this key.
            unsafe { std::env::set_var(key, value) };
            Self {
                key: key.to_string(),
                prev,
            }
        }
    }

    impl Drop for ScopedEnv {
        fn drop(&mut self) {
            // Safety: see `set` above.
            match &self.prev {
                Some(v) => unsafe { std::env::set_var(&self.key, v) },
                None => unsafe { std::env::remove_var(&self.key) },
            }
        }
    }

    /// D-02: an empty workspace string is rejected before any filesystem access.
    #[test]
    fn resolve_workspace_dir_rejects_empty() {
        assert!(resolve_workspace_dir("").is_err());
    }

    /// D-02: a workspace string containing a literal '$' (unexpanded shell
    /// variable — the exact incident symptom) is rejected.
    #[test]
    fn resolve_workspace_dir_rejects_dollar_sign() {
        let err = resolve_workspace_dir("$IRONHERMES_KANBAN_WORKSPACE").unwrap_err();
        assert!(
            err.to_string().contains('$') || err.to_string().to_lowercase().contains("unexpanded"),
            "error message should mention the unexpanded-variable condition, got: {err}"
        );
    }

    /// D-02: a relative path is rejected (must be absolute).
    #[test]
    fn resolve_workspace_dir_rejects_relative() {
        assert!(resolve_workspace_dir("foo/bar").is_err());
    }

    /// D-02: a non-existent absolute directory (outside the scratch root, so
    /// it is never auto-created) is rejected.
    #[test]
    fn resolve_workspace_dir_rejects_missing_absolute_dir() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("does-not-exist-subdir");
        assert!(!missing.exists());
        assert!(resolve_workspace_dir(missing.to_str().unwrap()).is_err());
    }

    /// A real, existing absolute directory resolves to Ok with a canonical path.
    #[test]
    fn resolve_workspace_dir_accepts_real_absolute_dir() {
        let dir = tempfile::tempdir().unwrap();
        let resolved = resolve_workspace_dir(dir.path().to_str().unwrap())
            .expect("existing absolute dir should resolve Ok");
        assert!(resolved.is_absolute());
        assert!(resolved.is_dir());
    }

    /// D-31: a scratch path under `kanban_workspaces_root()` is created
    /// eagerly and resolves to Ok even though it did not exist beforehand.
    #[test]
    fn resolve_workspace_dir_creates_scratch_path_under_workspaces_root() {
        let home_dir = tempfile::tempdir().unwrap();
        let _guard = ScopedEnv::set("IRONHERMES_HOME", home_dir.path().to_str().unwrap());

        let scratch = kanban_workspaces_root().join("t_scratch_test");
        assert!(!scratch.exists(), "scratch dir must not pre-exist");

        let resolved = resolve_workspace_dir(scratch.to_str().unwrap())
            .expect("scratch path under workspaces root should resolve Ok and be created");
        assert!(resolved.is_dir(), "scratch dir must be created on disk");
    }
}
