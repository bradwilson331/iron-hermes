//! Full ACP session lifecycle (Phase 36.8, plan 02): the six CLI-04 operations
//! (`create`/`get`/`remove`/`fork`/`list`/`cleanup`), `StateStore` write-through (D-10),
//! per-session `AgentRuntime` cwd binding (CLI-08, RESEARCH Pitfall 2), and a fresh
//! per-session `ApprovalsStore` (D-14, RESEARCH Pitfall 5 — never a process-wide shared
//! one). `close()` (D-13) additionally archives memory on session end; it is not one of
//! the six CLI-04 operations but is required for parity with CLI/gateway session teardown.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use ironhermes_agent::memory::factory::build_memory_manager;
use ironhermes_agent::subagent_registry::SubagentRegistry;
use ironhermes_agent::{AgentRuntime, AgentRuntimeInput, MemoryManager};
use ironhermes_core::commands::context::TrajectoryWriterHandle;
use ironhermes_core::{ApprovalsStore, ChatMessage, Config, MemoryEntries, ProviderResolver, Role};
use ironhermes_exec::process_registry::ProcessRegistry;
use ironhermes_hooks::HooksConfig;
use ironhermes_state::StateStore;
use ironhermes_trajectory::{TrajectoryWriter, TrajectoryWriterHandleImpl};
use tokio::sync::{Mutex as TokioMutex, RwLock};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

/// Source tag every ACP session is recorded under in `StateStore` (D-10).
pub const ACP_SESSION_SOURCE: &str = "acp";

/// D-11 / T-36.8-08: bounded resume rehydration cap. Mirrors
/// `ironhermes_gateway::session::SessionStore::MAX_RESUME_REHYDRATE_MESSAGES` verbatim —
/// `session/load` never hands the context engine an unbounded history window.
pub const MAX_RESUME_REHYDRATE_MESSAGES: usize = 200;

/// D-11: bounded head-aligned rehydration, reused verbatim from the gateway's
/// `SessionStore::build_or_resume_session` truncation path
/// (`crates/ironhermes-gateway/src/session.rs:186-335`). Pure function so `tests/
/// session_load.rs` can exercise the exact 200-bound / head-alignment logic without
/// standing up a live `AgentRuntime`.
///
/// - `messages.len() <= MAX_RESUME_REHYDRATE_MESSAGES`: returned unchanged (byte-identical).
/// - Over the cap: drains the front down to the most recent `MAX_RESUME_REHYDRATE_MESSAGES`,
///   then advances the head to the first [`Role::User`] in that window so no orphaned
///   tool_result/assistant message leads the rehydrated history.
/// - If the retained window has no [`Role::User`] at all: retains nothing rather than
///   handing the provider a protocol-invalid orphaned head. `StateStore` still has
///   everything — only the in-memory rehydration is bounded.
pub fn bounded_head_aligned_rehydrate(mut messages: Vec<ChatMessage>) -> Vec<ChatMessage> {
    let total_before = messages.len();
    if total_before > MAX_RESUME_REHYDRATE_MESSAGES {
        let drop_front = total_before - MAX_RESUME_REHYDRATE_MESSAGES;
        messages.drain(0..drop_front);
        match messages.iter().position(|m| m.role == Role::User) {
            Some(head_idx) => {
                if head_idx > 0 {
                    messages.drain(0..head_idx);
                }
            }
            None => {
                messages.clear();
            }
        }
    }
    messages
}

/// One ACP session: a per-session `AgentRuntime` (RESEARCH Pattern 1 — built fresh at
/// `session/new`/`session/load`/`fork`, never shared across sessions in the same process)
/// plus the bits `session/prompt`, `session/cancel`, and the permission bridge need.
pub struct AcpSession {
    pub runtime: AgentRuntime,
    pub cwd: PathBuf,
    pub session_id: String,
    pub cancel_token: CancellationToken,
    /// D-14 / RESEARCH Pitfall 5: a FRESH store per session — the session tier is
    /// keyed by normalized command text, not session id, so sharing one process-wide
    /// store would let an `allow_always` grant in one editor window silently suppress
    /// the same prompt in another.
    pub approvals: Arc<ApprovalsStore>,
    /// D-18: lazily opened per-session trajectory writer, consumed by plan 03.
    pub trajectory_writer: Option<Arc<dyn TrajectoryWriterHandle>>,
    pub created_at: Instant,
    /// Set by `close()`; `cleanup()` removes sessions where this is `true`.
    pub closed: bool,
    /// Conversation history for this session — empty for a freshly `create()`d session,
    /// seeded with the bounded head-aligned rehydration on `load()` (D-11) or copied
    /// verbatim on `fork()`.
    pub messages: Vec<ChatMessage>,
    /// Kept alongside (not instead of) the copy handed to `AgentRuntime::from_config` —
    /// `AgentRuntime` has no public accessor for its own memory manager, and `close()`
    /// needs one to fire the D-13 archive-on-end hook.
    memory_manager: Option<Arc<TokioMutex<MemoryManager>>>,
}

/// In-memory `session_id -> AcpSession` map, one instance shared (behind an
/// `Arc<TokioMutex<..>>`, owned by `entry::run_acp_over`) across every handler for the
/// lifetime of the `ironhermes acp` process. Owns the `StateStore` write-through handle
/// (D-10) and the process-wide `Config`/`ProviderResolver` every session's `AgentRuntime`
/// is built from.
pub struct AcpSessionManager {
    sessions: HashMap<String, AcpSession>,
    /// Creation order, maintained explicitly so `list()` is stable across repeated
    /// calls rather than `HashMap`-arbitrary (plan 02 task 1 acceptance criterion).
    order: Vec<String>,
    state: Arc<Mutex<StateStore>>,
    config: Arc<Config>,
    resolver: Arc<ProviderResolver>,
    /// Plan 03 task 3 (T-36.8-16): the CURRENT per-turn `CancellationToken` for every
    /// live session, mirrored OUTSIDE this manager's own `Arc<TokioMutex<..>>` on
    /// purpose. `handle_session_prompt` holds that outer lock for the FULL duration of
    /// an in-flight `run_turn` (plan 01's documented "keep session_manager.rs minimal"
    /// simplification) — a `session/cancel` handler that had to acquire the same lock
    /// could therefore never fire a token until the very turn it's trying to cancel had
    /// already finished on its own, defeating cancellation entirely. This
    /// `std::sync::Mutex`-guarded map is cheap to acquire (never held across an
    /// `.await`) even while the big manager lock is held by a concurrent in-flight
    /// turn, so `session/cancel` can interrupt it. Kept in sync with session lifecycle:
    /// inserted in `build_and_insert` (and refreshed by `handlers::handle_session_prompt`
    /// each time it mints a fresh per-turn token), removed in `remove`/`cleanup`.
    cancel_tokens: Arc<Mutex<HashMap<String, CancellationToken>>>,
}

impl AcpSessionManager {
    pub fn new(state: Arc<Mutex<StateStore>>, config: Arc<Config>, resolver: Arc<ProviderResolver>) -> Self {
        Self {
            sessions: HashMap::new(),
            order: Vec::new(),
            state,
            config,
            resolver,
            cancel_tokens: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Plan 03 task 3: the shared, lock-cheap per-session cancellation-token map — clone
    /// once at server startup (`entry::run_acp_over`) so the `session/cancel` notification
    /// handler never needs this manager's own `Arc<TokioMutex<..>>` at all.
    pub fn cancel_tokens(&self) -> Arc<Mutex<HashMap<String, CancellationToken>>> {
        self.cancel_tokens.clone()
    }

    /// Build a fresh per-session `AgentRuntime` + `ApprovalsStore` and insert it under
    /// `session_id` (already allocated by the caller — either a fresh uuid from
    /// `create()`, an existing id being rehydrated by `load()`, or a fresh uuid from
    /// `fork()`). Does NOT touch `StateStore` — callers own the row (create/write vs.
    /// load/read-only vs. fork/write-with-parent-linkage). `seed_messages` becomes the
    /// session's initial `messages` buffer (empty for `create()`, rehydrated history for
    /// `load()`/`fork()`).
    async fn build_and_insert(
        &mut self,
        session_id: String,
        cwd: PathBuf,
        seed_messages: Vec<ChatMessage>,
    ) -> anyhow::Result<()> {
        let memory_manager = build_memory_manager(&self.config.memory).await?;

        let process_registry = Arc::new(RwLock::new(ProcessRegistry::new_for_session(
            session_id.clone(),
        )));
        let subagent_registry = Arc::new(RwLock::new(SubagentRegistry::new()));
        let hermes_home = ironhermes_core::get_hermes_home();

        let runtime = AgentRuntime::from_config(AgentRuntimeInput {
            config: self.config.clone(),
            resolver: self.resolver.clone(),
            cwd: cwd.clone(),
            process_registry,
            memory_manager: memory_manager.clone(),
            hooks_config: HooksConfig::load().unwrap_or_default(),
            emit_mcp_startup_logs: false,
            subagent_registry,
            transcript_scope: (hermes_home, session_id.clone()),
            subagent_progress_callback: None,
            subagent_cancel_token: None,
        })
        .await?;

        let cancel_token = CancellationToken::new();
        let session = AcpSession {
            runtime,
            cwd,
            session_id: session_id.clone(),
            cancel_token: cancel_token.clone(),
            approvals: Arc::new(ApprovalsStore::new()),
            trajectory_writer: None,
            created_at: Instant::now(),
            closed: false,
            messages: seed_messages,
            memory_manager,
        };

        // Keep the lock-cheap cancel-token mirror in sync (see field doc on
        // `AcpSessionManager::cancel_tokens`).
        self.cancel_tokens
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(session_id.clone(), cancel_token);

        self.sessions.insert(session_id.clone(), session);
        self.order.push(session_id);
        Ok(())
    }

    /// CLI-04 `create`: allocate a fresh session id, build its per-session `AgentRuntime`
    /// bound to `cwd` (CLI-08), and write-through a `StateStore` row with source `acp`
    /// (D-10). On a poisoned `StateStore` lock, degrades to an unpersisted in-memory
    /// session (gateway `build_or_resume_session` precedent) rather than panicking.
    pub async fn create(&mut self, cwd: PathBuf, model: Option<&str>) -> anyhow::Result<String> {
        let session_id = format!("acp_{}", Uuid::new_v4());
        self.build_and_insert(session_id.clone(), cwd.clone(), Vec::new())
            .await?;

        let canonical_cwd = cwd.to_string_lossy().into_owned();
        match self.state.lock() {
            Ok(mut state) => {
                if let Err(e) = state.create_session(
                    &session_id,
                    ACP_SESSION_SOURCE,
                    model,
                    None,
                    None,
                    Some(&canonical_cwd),
                ) {
                    tracing::warn!(
                        error = %e,
                        session_id = %session_id,
                        "ACP session: failed to persist session to StateStore"
                    );
                }
            }
            Err(_) => {
                tracing::warn!(
                    session_id = %session_id,
                    "state lock poisoned — ACP session {session_id} will be unpersisted in-memory only"
                );
            }
        }

        Ok(session_id)
    }

    /// `session/load` support (D-11): idempotent lookup-or-rehydrate. If `session_id` is
    /// already live in this process, returns immediately without touching `StateStore` —
    /// "loading the same session id twice returns the same live session and does not
    /// double-rehydrate the history buffer." Otherwise reads the session's `StateStore`
    /// row (erroring if unknown — a stale editor session must never silently lose its
    /// history by minting a fresh session under the requested id) and its messages,
    /// applies [`bounded_head_aligned_rehydrate`], and builds a fresh `AcpSession`
    /// rebinding the runtime to `cwd` (the editor may reopen the project elsewhere).
    pub async fn load(&mut self, session_id: &str, cwd: PathBuf) -> anyhow::Result<()> {
        if self.sessions.contains_key(session_id) {
            return Ok(());
        }

        let rehydrated = {
            let state = self
                .state
                .lock()
                .map_err(|_| anyhow::anyhow!("state lock poisoned"))?;
            if state.get_session(session_id)?.is_none() {
                anyhow::bail!("unknown session: {session_id}");
            }
            let messages = state.get_chat_messages(session_id)?;
            bounded_head_aligned_rehydrate(messages)
        };

        self.build_and_insert(session_id.to_string(), cwd, rehydrated)
            .await
    }

    /// CLI-04 `get`.
    pub fn get(&self, session_id: &str) -> Option<&AcpSession> {
        self.sessions.get(session_id)
    }

    /// CLI-04 `get` (mutable).
    pub fn get_mut(&mut self, session_id: &str) -> Option<&mut AcpSession> {
        self.sessions.get_mut(session_id)
    }

    /// D-10 / plan 03 task 2: shared `StateStore` handle, for `TurnRequest.state_store`
    /// (`session_search` tool interception) and for the caller to persist a turn's
    /// user/assistant messages after `run_turn` returns. Cloning the `Arc` is cheap and
    /// lets the caller use the store without holding this manager's lock across the turn.
    pub fn state_store(&self) -> Arc<Mutex<StateStore>> {
        self.state.clone()
    }

    /// Plan 03 task 2: the shared `ProviderResolver`, so the caller can resolve the
    /// current model's context window size for the post-turn usage update without this
    /// manager exposing raw `Config`/`ProviderResolver` internals more broadly.
    pub fn resolver(&self) -> Arc<ProviderResolver> {
        self.resolver.clone()
    }

    /// Plan 04 task 3: the shared `Config`, so the caller can build a
    /// `DangerousCommandGuardrail` from `config.dangerous_commands` and read
    /// `config.approvals.timeout_secs` / `config.autonomous.yolo` / `config.audit` when
    /// wiring the per-turn approval gate and `terminal`/`execute_code` intercepts.
    pub fn config(&self) -> Arc<Config> {
        self.config.clone()
    }

    /// D-18 / plan 03 task 2: get or lazily-open a per-session `TrajectoryWriter` handle,
    /// mirroring `ironhermes_gateway::session::SessionStore::get_or_create_trajectory_writer`
    /// verbatim — `<session.cwd>/.ironhermes/sessions/<session_id>/trajectories.jsonl`, one
    /// file handle per session reused across turns. Returns `None` for an unknown session id
    /// or if opening the writer failed (logged; best-effort — a trajectory-open failure must
    /// never fail the turn itself).
    pub fn get_or_create_trajectory_writer(
        &mut self,
        session_id: &str,
    ) -> Option<Arc<dyn TrajectoryWriterHandle>> {
        let session = self.sessions.get_mut(session_id)?;
        if let Some(existing) = &session.trajectory_writer {
            return Some(existing.clone());
        }

        let traj_path = session
            .cwd
            .join(".ironhermes")
            .join("sessions")
            .join(session_id)
            .join("trajectories.jsonl");

        match TrajectoryWriter::open(&traj_path) {
            Ok(writer) => {
                let arc_writer = Arc::new(Mutex::new(writer));
                let handle: Arc<dyn TrajectoryWriterHandle> =
                    Arc::new(TrajectoryWriterHandleImpl::new(arc_writer));
                session.trajectory_writer = Some(handle.clone());
                Some(handle)
            }
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    path = %traj_path.display(),
                    session_id = %session_id,
                    "ACP session: failed to open per-session trajectory writer (D-18); \
                     per-tool-call ledger disabled for this turn"
                );
                None
            }
        }
    }

    /// CLI-04 `remove`: drops the in-memory session and ends the `StateStore` row.
    /// Idempotent — a second call on an already-removed (or never-existing) id returns
    /// `false` as a no-op success rather than an error.
    pub fn remove(&mut self, session_id: &str) -> bool {
        let existed = self.sessions.remove(session_id).is_some();
        if existed {
            self.order.retain(|id| id != session_id);
            self.cancel_tokens
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(session_id);
            match self.state.lock() {
                Ok(mut state) => {
                    if let Err(e) = state.end_session(session_id, "acp_session_removed") {
                        tracing::warn!(error = %e, session_id = %session_id, "ACP session remove: end_session failed");
                    }
                }
                Err(_) => {
                    tracing::warn!(session_id = %session_id, "state lock poisoned during ACP session remove");
                }
            }
        }
        existed
    }

    /// CLI-04 `list`: live sessions in creation order. Empty manager -> empty vec, never
    /// an error. Order is stable across repeated calls (backed by an explicit `Vec`, not
    /// `HashMap` iteration).
    pub fn list(&self) -> Vec<String> {
        self.order.clone()
    }

    /// CLI-04 `cleanup`: removes sessions already marked `closed` (via `close()`). Safe
    /// to call on an empty manager or when nothing is closed.
    pub fn cleanup(&mut self) {
        let closed_ids: Vec<String> = self
            .sessions
            .iter()
            .filter(|(_, session)| session.closed)
            .map(|(id, _)| id.clone())
            .collect();
        for id in closed_ids {
            self.sessions.remove(&id);
            self.order.retain(|existing| existing != &id);
            self.cancel_tokens
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(&id);
        }
    }

    /// D-13 (not one of the six CLI-04 operations, but required for session teardown
    /// parity with CLI/gateway): marks the session closed, archives its memory via the
    /// memory manager's `on_session_end` hook (best-effort, matching the CLI's own
    /// GAP-6 `run_single` pattern), and ends the `StateStore` row. Returns `false` for
    /// an unknown id rather than panicking.
    pub async fn close(&mut self, session_id: &str) -> bool {
        let memory_manager = match self.sessions.get_mut(session_id) {
            Some(session) => {
                session.closed = true;
                session.memory_manager.clone()
            }
            None => return false,
        };

        if let Some(mgr) = memory_manager {
            let mgr_lock = mgr.lock().await;
            let entries = MemoryEntries::default();
            if let Err(e) = mgr_lock.on_session_end(session_id, &entries).await {
                tracing::debug!(
                    error = %e,
                    session_id = %session_id,
                    "ACP session close: on_session_end failed (best-effort)"
                );
            }
        }

        match self.state.lock() {
            Ok(mut state) => {
                if let Err(e) = state.end_session(session_id, "acp_session_closed") {
                    tracing::warn!(error = %e, session_id = %session_id, "ACP session close: end_session failed");
                }
            }
            Err(_) => {
                tracing::warn!(session_id = %session_id, "state lock poisoned during ACP session close");
            }
        }

        true
    }

    /// CLI-04 `fork`: copies `source_id`'s history into a fresh, collision-free session
    /// id with parent linkage (D-11). Application-level composition of existing
    /// `StateStore` primitives (RESEARCH Pitfall 3 — there is NO `StateStore::fork_session`
    /// method; do not add one): read the source's messages via `get_chat_messages`,
    /// allocate a fresh id guarded against collision with both the in-memory map and
    /// `StateStore`, `create_session` the child row with `parent_session_id =
    /// Some(source_id)`, replay the copied messages into it via `add_message`, then build
    /// the child's OWN `AgentRuntime` and `ApprovalsStore` bound to the source's cwd — a
    /// fork must never inherit the parent's `allow_always` grants (trust granted in one
    /// conversation does not transfer to a branch of it).
    ///
    /// The source's cwd is read from the live in-memory session if present, else
    /// reconstructed from the source row's `workspace_root`. Errors (rather than panics)
    /// on an unknown source id, or when neither a live session nor a recorded
    /// `workspace_root` can supply a cwd to bind the child to.
    ///
    /// Note (task 3, "wire fork into the handler surface"): the pinned
    /// `agent-client-protocol-schema` 1.5.0 exposes NO fork/branch JSON-RPC method in its
    /// stable surface — the real one is gated behind the `unstable_session_fork` cargo
    /// feature (not enabled here), and `NewSessionRequest` carries no parent-session
    /// field to overload for this purpose. `fork` therefore exists as a manager-level
    /// operation only, reachable from within this crate (and by a future plan once the
    /// SDK's fork RPC stabilizes), with no current ACP wire-level trigger.
    pub async fn fork(&mut self, source_id: &str) -> anyhow::Result<String> {
        let (source_cwd, source_model, source_system_prompt, workspace_root, source_messages) = {
            let state = self
                .state
                .lock()
                .map_err(|_| anyhow::anyhow!("state lock poisoned"))?;
            let Some(row) = state.get_session(source_id)? else {
                anyhow::bail!("unknown source session: {source_id}");
            };
            let messages = state.get_chat_messages(source_id)?;
            let live_cwd = self.sessions.get(source_id).map(|s| s.cwd.clone());
            let cwd = live_cwd
                .or_else(|| row.workspace_root.as_ref().map(PathBuf::from))
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "cannot determine cwd for source session {source_id} (not live \
                         in-process and no workspace_root recorded in StateStore)"
                    )
                })?;
            (cwd, row.model.clone(), row.system_prompt.clone(), row.workspace_root.clone(), messages)
        };

        // CLI-04 adjacency contract: a forked id must never merge into or collide with
        // an existing session, in-memory or persisted. Loop rather than trusting uuid
        // uniqueness alone.
        let new_id = loop {
            let candidate = format!("acp_{}", Uuid::new_v4());
            let collides_in_memory = self.sessions.contains_key(&candidate);
            let collides_in_state = {
                let state = self
                    .state
                    .lock()
                    .map_err(|_| anyhow::anyhow!("state lock poisoned"))?;
                state.get_session(&candidate)?.is_some()
            };
            if !collides_in_memory && !collides_in_state {
                break candidate;
            }
        };

        {
            let mut state = self
                .state
                .lock()
                .map_err(|_| anyhow::anyhow!("state lock poisoned"))?;
            state.create_session(
                &new_id,
                ACP_SESSION_SOURCE,
                source_model.as_deref(),
                source_system_prompt.as_deref(),
                Some(source_id),
                workspace_root.as_deref(),
            )?;
            for message in &source_messages {
                state.add_message(&new_id, message)?;
            }
        }

        self.build_and_insert(new_id.clone(), source_cwd, source_messages)
            .await?;

        Ok(new_id)
    }
}
