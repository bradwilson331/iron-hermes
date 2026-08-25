//! Translates the three `AgentRuntime::run_turn` per-turn callbacks (`stream`,
//! `tool_progress`, `tool_result`) into ACP `session/update` notifications (CLI-05).
//!
//! # Design
//!
//! The callback aliases (`StreamCallback`/`ToolProgressCallback`/`ToolResultCallback`)
//! are synchronous `Fn` — they run inline inside `AgentLoop::run` and must never block or
//! `.await`. [`AcpEventBridge`] therefore does the tool-call-id translation work
//! synchronously inside each callback (cheap `Mutex`-guarded map lookups, deliberately held
//! across the channel send — see `ToolCallIdAllocator`'s doc for why that ordering is what
//! keeps a keepalive heartbeat from overtaking a terminal update) and enqueues the
//! resulting [`SessionUpdate`] onto a single
//! `tokio::sync::mpsc::UnboundedSender` shared by all three callback kinds. A spawned
//! task drains that one channel and forwards each update as a `session/update`
//! notification via a [`NotificationSink`] — one channel for all three callback kinds so
//! chunk and tool updates interleave in exactly the order the agent loop produced them.
//!
//! [`NotificationSink`] abstracts over a live `ConnectionTo<Client>` (production) and a
//! collecting fake (tests), so `tests/event_bridge.rs` can assert on translation behavior
//! without standing up a real connection.
//!
//! # Scope (COVERAGE.md)
//!
//! This module emits exactly the `SessionUpdate` variants COVERAGE.md marks INTEGRATE for
//! per-turn callback translation: `agent_message_chunk`, `tool_call`, `tool_call_update`.
//! `usage_update` is also INTEGRATE but is not sourced from a per-turn callback — plan 03
//! task 2 sends it once, after `run_turn` returns, through [`AcpEventBridge::send_update`]
//! so it shares this module's ordering guarantee with whatever is still draining.
//!
//! No reasoning-shaped update (the one COVERAGE.md marks OPT-OUT) is ever constructed
//! here: there is no live reasoning stream in this backend, and fabricating one would
//! present invented agent reasoning to the user as if it were real.
//!
//! # Plan 03 (D-07): silent-long-tool-call keepalive
//!
//! `buzz-acp` resets its idle deadline generically on every valid JSON-RPC line it reads,
//! so any of the three callback-sourced updates above already keeps a busy turn alive
//! (RESEARCH.md Pattern 1). The one real gap is a single tool call that runs a long time
//! with zero intermediate `tool_progress` ticks — nothing flows during that window. This
//! module closes it with a self-terminating heartbeat ticker (spawned in
//! [`AcpEventBridge::new`] alongside the drain task) that emits a typed `ToolCallUpdate`
//! (status `InProgress`, no content) on the in-flight tool call once a full
//! [`keepalive_interval`] has elapsed with no other traffic. There is deliberately **no**
//! literal `{"sessionUpdate":"keepalive"}` shape here — `SessionUpdate`
//! (`agent-client-protocol-schema` 1.5.0) is `#[non_exhaustive]` but has no `Keepalive`
//! variant and no raw-JSON escape hatch, and a typed `tool_call_update` is strictly better
//! for the client than that literal shape would be (RESEARCH.md Open Question 1 / this
//! plan's Design decision note).
//!
//! # Plan 05 (CLI-07): tool-call/result content
//!
//! `tool_progress_callback`'s second parameter carries the LLM's raw JSON tool-call
//! arguments string (see `ironhermes_agent::agent_loop::ToolProgressCallback`'s doc); this
//! module routes it through [`crate::tool_render::render_call_content`] (resolved against
//! this bridge's `cwd`) to populate the `tool_call`/`tool_call_update`'s `content` and
//! `kind`. `tool_result_callback`'s third parameter carries the tool's real output text,
//! routed through [`crate::tool_render::render_result_content`].

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use agent_client_protocol::schema::v1::{
    ContentBlock, ContentChunk, SessionId, SessionNotification, SessionUpdate, TextContent,
    ToolCall, ToolCallContent, ToolCallStatus, ToolCallUpdate, ToolCallUpdateFields,
};
use agent_client_protocol::{Client, ConnectionTo};
use ironhermes_agent::agent_loop::{StreamCallback, ToolProgressCallback, ToolResultCallback};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio::time::Instant;

/// Default heartbeat interval (seconds) for the self-terminating keepalive ticker spawned
/// in [`AcpEventBridge::new`] — safely under buzz-acp's `DEFAULT_IDLE_TIMEOUT_SECS = 900`
/// (RESEARCH.md Pitfall 2 / D-07), and still under a conservatively lowered operator value.
const KEEPALIVE_INTERVAL_SECS: u64 = 60;

/// Env var overriding [`KEEPALIVE_INTERVAL_SECS`]. An explicit `0` disables the heartbeat
/// entirely; unset or unparseable falls back to the default (an unparseable value logs one
/// `tracing::warn!`).
const KEEPALIVE_INTERVAL_ENV: &str = "IRONHERMES_ACP_KEEPALIVE_SECS";

/// Resolves the heartbeat interval from [`KEEPALIVE_INTERVAL_ENV`]. Returns `None` only for
/// an explicit `0` (heartbeat disabled); unset or unparseable values fall back to
/// `Some(KEEPALIVE_INTERVAL_SECS)`.
fn keepalive_interval() -> Option<Duration> {
    match std::env::var(KEEPALIVE_INTERVAL_ENV) {
        Ok(raw) => match raw.parse::<u64>() {
            Ok(0) => None,
            Ok(secs) => Some(Duration::from_secs(secs)),
            Err(_) => {
                tracing::warn!(
                    value = %raw,
                    env = KEEPALIVE_INTERVAL_ENV,
                    default_secs = KEEPALIVE_INTERVAL_SECS,
                    "keepalive interval env var is not a valid u64; using default"
                );
                Some(Duration::from_secs(KEEPALIVE_INTERVAL_SECS))
            }
        },
        Err(_) => Some(Duration::from_secs(KEEPALIVE_INTERVAL_SECS)),
    }
}

/// Sends `update` on `tx` and records the moment of the send into `last_emit`. ALL emission
/// sites (the three callbacks, `send_update`, and the keepalive ticker itself) route
/// through this single function so the ticker's "has anything else flowed since my last
/// check" gate stays accurate regardless of which site produced the most recent traffic.
fn enqueue(
    tx: &mpsc::UnboundedSender<SessionUpdate>,
    last_emit: &Arc<Mutex<Instant>>,
    update: SessionUpdate,
) {
    let _ = tx.send(update);
    let mut guard = last_emit.lock().unwrap_or_else(|e| e.into_inner());
    *guard = Instant::now();
}

/// Anything the bridge can push a `session/update` notification to. Abstracts over a live
/// `ConnectionTo<Client>` (production) vs. a collecting fake (tests), so the translation
/// logic in this module never has to stand up a real connection to be tested.
pub trait NotificationSink: Send + Sync {
    fn send(&self, notification: SessionNotification);
}

/// Production sink: forwards straight onto the real ACP connection. A closed/errored
/// connection means there's no client left to notify — the turn itself still completes,
/// which is fine; the client is already gone.
impl NotificationSink for ConnectionTo<Client> {
    fn send(&self, notification: SessionNotification) {
        let _ = self.send_notification(notification);
    }
}

/// Interior-mutable map from tool name to the currently in-flight `tool_call_id`, plus a
/// monotonic counter minting fresh ids of the form `call_<n>`.
///
/// # Why every emission happens under the lock
///
/// Each method here both mutates the map AND performs its channel send while holding the
/// lock, rather than returning an id for the caller to send with afterwards. That ordering
/// is the whole point: the keepalive ticker runs on its own task while the three callbacks
/// run on the agent-loop task, so any gap between "read the in-flight id" and "send an
/// update naming it" is a window in which the two can interleave. The damaging interleaving
/// is a heartbeat landing AFTER a terminal update:
///
/// 1. ticker reads `call_3` as in flight, releases the lock,
/// 2. agent loop sends `Completed` for `call_3` and clears the entry,
/// 3. ticker sends `InProgress` for `call_3`.
///
/// Both sends share one channel, so the client receives `Completed` then `InProgress` for
/// the same id — and because the entry is now cleared, nothing further is ever sent for it.
/// An editor tracking tool-call state renders a permanent spinner on a call that finished.
/// Closing the gap on only one side does not help: whichever side releases the lock before
/// sending is the side that can be overtaken.
///
/// Holding a `std::sync::Mutex` across `UnboundedSender::send` is safe — the send never
/// blocks and never `.await`s — and the lock is uncontended in practice. Lock ordering is
/// always `in_flight` then `last_emit` (via [`enqueue`]); nothing acquires them the other
/// way round.
struct ToolCallIdAllocator {
    in_flight: Mutex<HashMap<String, String>>,
    next: AtomicU64,
}

impl ToolCallIdAllocator {
    fn new() -> Self {
        Self {
            in_flight: Mutex::new(HashMap::new()),
            next: AtomicU64::new(0),
        }
    }

    /// Emits a progress update for `tool_name`: a `ToolCall` the first time the name has no
    /// in-flight id (allocating and recording a fresh one), a `ToolCallUpdate` on the same
    /// id for every subsequent progress event.
    fn progress(
        &self,
        tool_name: &str,
        tx: &mpsc::UnboundedSender<SessionUpdate>,
        last_emit: &Arc<Mutex<Instant>>,
        content: Vec<ToolCallContent>,
    ) {
        let mut guard = self.in_flight.lock().unwrap_or_else(|e| e.into_inner());
        let update = match guard.get(tool_name) {
            Some(existing) => SessionUpdate::ToolCallUpdate(ToolCallUpdate::new(
                existing.clone(),
                ToolCallUpdateFields::new()
                    .status(ToolCallStatus::InProgress)
                    .content(content),
            )),
            None => {
                let id = self.mint();
                guard.insert(tool_name.to_string(), id.clone());
                SessionUpdate::ToolCall(
                    ToolCall::new(id, human_readable_title(tool_name))
                        .kind(crate::tool_render::tool_kind(tool_name))
                        .status(ToolCallStatus::InProgress)
                        .content(content),
                )
            }
        };
        enqueue(tx, last_emit, update);
    }

    /// Emits the terminal update for `tool_name` and clears its in-flight entry in the SAME
    /// lock acquisition, so no heartbeat can slip in between the send and the clear. A
    /// result with no matching in-flight `tool_call` emits a synthetic `tool_call` followed
    /// immediately by its terminal update rather than being dropped; that id is minted here
    /// and deliberately never recorded, since it is already terminal.
    fn finish(
        &self,
        tool_name: &str,
        tx: &mpsc::UnboundedSender<SessionUpdate>,
        last_emit: &Arc<Mutex<Instant>>,
        status: ToolCallStatus,
        content: Vec<ToolCallContent>,
    ) {
        let mut guard = self.in_flight.lock().unwrap_or_else(|e| e.into_inner());
        let id = match guard.remove(tool_name) {
            Some(id) => id,
            None => {
                let id = self.mint();
                enqueue(
                    tx,
                    last_emit,
                    SessionUpdate::ToolCall(
                        ToolCall::new(id.clone(), human_readable_title(tool_name))
                            .kind(crate::tool_render::tool_kind(tool_name))
                            .status(ToolCallStatus::InProgress),
                    ),
                );
                id
            }
        };
        enqueue(
            tx,
            last_emit,
            SessionUpdate::ToolCallUpdate(ToolCallUpdate::new(
                id,
                ToolCallUpdateFields::new().status(status).content(content),
            )),
        );
    }

    /// Emits one `InProgress` heartbeat on a currently-in-flight tool call, chosen
    /// deterministically (the entry whose tool name sorts first alphabetically) so the
    /// ticker never oscillates between ids when two tools are in flight at once. Emits
    /// nothing when nothing is in flight — the bridge never invents a running tool call
    /// just to keep the connection warm. The in-flight check and the send share one lock
    /// acquisition (see the type doc).
    fn heartbeat_if_in_flight(
        &self,
        tx: &mpsc::UnboundedSender<SessionUpdate>,
        last_emit: &Arc<Mutex<Instant>>,
    ) {
        let guard = self.in_flight.lock().unwrap_or_else(|e| e.into_inner());
        let Some((_, id)) = guard.iter().min_by_key(|(name, _)| name.as_str()) else {
            return;
        };
        enqueue(
            tx,
            last_emit,
            SessionUpdate::ToolCallUpdate(ToolCallUpdate::new(
                id.clone(),
                ToolCallUpdateFields::new().status(ToolCallStatus::InProgress),
            )),
        );
    }

    /// Next fresh id. Callers hold the `in_flight` lock; the counter is atomic so this is
    /// still correct for the one caller ([`Self::finish`]'s no-matching-call arm) that
    /// mints an id it never records.
    fn mint(&self) -> String {
        format!("call_{}", self.next.fetch_add(1, Ordering::SeqCst))
    }
}

/// A short, human-readable title for a tool call, derived from the tool name only (no
/// phase/status text — this plan emits envelopes with title and status only; content
/// rendering is plan 05's job).
fn human_readable_title(tool_name: &str) -> String {
    format!("Running {tool_name}")
}

/// Translates one ACP session's `AgentRuntime` callbacks into `session/update`
/// notifications. One instance per `session/prompt` call (built fresh each turn — the
/// underlying channel and tool-call-id allocator are per-turn state, not per-session).
pub struct AcpEventBridge {
    session_id: SessionId,
    tx: mpsc::UnboundedSender<SessionUpdate>,
    allocator: Arc<ToolCallIdAllocator>,
    /// Plan 05 (CLI-07): the session's cwd, used to resolve relative paths when rendering
    /// file-edit diffs (`write_file`/`patch`) via `tool_render::render_call_content`.
    cwd: PathBuf,
    /// Plan 03 (D-07): the moment of the most recent emission through [`enqueue`] (any
    /// site), read by the keepalive ticker to decide whether other traffic already reset
    /// the client's idle deadline since its last check.
    last_emit: Arc<Mutex<Instant>>,
    /// D-12: which tool calls this crate's approval gate refused, recorded by the gate
    /// sites themselves. `tool_result_callback` consumes it to decide whether a failed
    /// result renders as a policy denial — see [`crate::handlers::DenialLedger`] for why
    /// that must not be inferred from the tool's own output text.
    denials: crate::handlers::DenialLedger,
}

impl AcpEventBridge {
    /// Builds a bridge bound to `session_id` and spawns the task that drains its internal
    /// channel into `sink`. Returns the bridge plus the drain task's `JoinHandle` —
    /// production callers drop the handle (the task runs to completion once every
    /// `AcpEventBridge`/callback clone holding a sender is dropped); tests hold it so they
    /// can `.await` deterministic completion before asserting on a collecting sink.
    ///
    /// `cwd` (plan 05, CLI-07) is the session's working directory, used to resolve
    /// relative file paths when rendering `write_file`/`patch` tool calls as diffs.
    ///
    /// Plan 03 (D-07): when [`keepalive_interval`] resolves to `Some(interval)`, this also
    /// spawns a second, self-terminating ticker task that emits a `tool_call_update`
    /// heartbeat on the in-flight tool call after `interval` of silence. The ticker holds
    /// only a `WeakUnboundedSender` (`tx.downgrade()`) — never a strong clone — so it can
    /// never keep the channel open: once every strong sender (the bridge itself and every
    /// callback clone) is dropped, the drain task above still ends and `drain_handle.await`
    /// in `handlers.rs` still completes, regardless of whether the ticker is still asleep.
    /// The ticker notices via a failed `upgrade()` and exits on its next wake, within at
    /// most one interval of the bridge dropping. No `Drop` impl is needed or used to
    /// compensate for this — the weak handle is the entire lifecycle mechanism.
    pub fn new(
        sink: Arc<dyn NotificationSink>,
        session_id: impl Into<SessionId>,
        cwd: impl Into<PathBuf>,
        denials: crate::handlers::DenialLedger,
    ) -> (Self, JoinHandle<()>) {
        let (tx, mut rx) = mpsc::unbounded_channel::<SessionUpdate>();
        let session_id = session_id.into();
        let drain_session_id = session_id.clone();
        let drain_handle = tokio::spawn(async move {
            while let Some(update) = rx.recv().await {
                sink.send(SessionNotification::new(drain_session_id.clone(), update));
            }
        });

        let allocator = Arc::new(ToolCallIdAllocator::new());
        let last_emit = Arc::new(Mutex::new(Instant::now()));

        if let Some(interval) = keepalive_interval() {
            let weak_tx = tx.downgrade();
            let ticker_allocator = allocator.clone();
            let ticker_last_emit = last_emit.clone();
            tokio::spawn(async move {
                // Sleep length for the NEXT wake. Starts at a full interval and is
                // shortened to the remaining silence window whenever other traffic landed
                // mid-interval — sleeping another WHOLE interval there would pin wakes to a
                // fixed `k * interval` grid and make the real worst-case silence two full
                // intervals (traffic just after a grid point waits out the rest of that
                // interval plus all of the next one). The module doc promises a heartbeat
                // once a full interval has elapsed with no other traffic, and an operator
                // lowering `IRONHERMES_ACP_KEEPALIVE_SECS` to stay under buzz-acp's idle
                // timeout is entitled to that number rather than double it.
                let mut next_sleep = interval;
                loop {
                    tokio::time::sleep(next_sleep).await;

                    let Some(tx) = weak_tx.upgrade() else {
                        // Every strong sender (bridge + callback clones) is gone — the
                        // channel is already closing (or closed) on its own. Nothing left
                        // to heartbeat for; self-terminate.
                        break;
                    };

                    let elapsed = {
                        let guard = ticker_last_emit.lock().unwrap_or_else(|e| e.into_inner());
                        guard.elapsed()
                    };
                    if let Some(remaining) = interval.checked_sub(elapsed)
                        && !remaining.is_zero()
                    {
                        // Other traffic emitted since this tick started sleeping — that
                        // traffic already reset the client's idle deadline generically.
                        // Wait out only what is LEFT of the silence window it reset.
                        next_sleep = remaining;
                        continue;
                    }
                    next_sleep = interval;

                    ticker_allocator.heartbeat_if_in_flight(&tx, &ticker_last_emit);
                }
            });
        }

        (
            Self {
                session_id,
                tx,
                allocator,
                cwd: cwd.into(),
                last_emit,
                denials,
            },
            drain_handle,
        )
    }

    /// `stream(delta)` -> `agent_message_chunk` carrying the delta as text content.
    pub fn stream_callback(&self) -> StreamCallback {
        let tx = self.tx.clone();
        let last_emit = self.last_emit.clone();
        Box::new(move |chunk: &str| {
            enqueue(
                &tx,
                &last_emit,
                SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::Text(
                    TextContent::new(chunk.to_string()),
                ))),
            );
        })
    }

    /// `tool_progress(name, args_json)` -> a `tool_call` update the first time `name` has
    /// no in-flight id (allocating a fresh one), and a `tool_call_update` on the same id
    /// for every subsequent progress event for that tool. Plan 05 (CLI-07): `args_json`
    /// (the LLM's raw tool-call arguments string) is rendered through
    /// `tool_render::render_call_content` (resolved against this bridge's `cwd`) into the
    /// update's `content`, and `tool_render::tool_kind` sets the `kind` on the FIRST
    /// (`ToolCall`) update — `ToolCallUpdateFields` also carries `kind`, but only the
    /// initial `ToolCall` needs it; subsequent `ToolCallUpdate`s just refresh content.
    pub fn tool_progress_callback(&self) -> ToolProgressCallback {
        let tx = self.tx.clone();
        let allocator = self.allocator.clone();
        let cwd = self.cwd.clone();
        let last_emit = self.last_emit.clone();
        Box::new(move |name: &str, args_json: &str| {
            let content = crate::tool_render::render_call_content(name, args_json, &cwd);
            allocator.progress(name, &tx, &last_emit, content);
        })
    }

    /// `tool_result(name, ok, output)` -> a `tool_call_update` on the in-flight id for
    /// `name` with a completed (`ok`) or failed (`!ok`) status, then clears the in-flight
    /// entry so a later call to the same tool allocates a fresh id. A result with no
    /// matching in-flight `tool_call` emits a synthetic `tool_call` followed immediately by
    /// its terminal `tool_call_update` rather than being dropped. Plan 05 (CLI-07):
    /// `output` (the tool's real captured result text) is rendered through
    /// `tool_render::render_result_content` into the terminal update's `content` — an
    /// empty `output` still produces the terminal status update, just with no content
    /// items, so an editor waiting on this call for a status transition is never left
    /// hanging.
    pub fn tool_result_callback(&self) -> ToolResultCallback {
        let tx = self.tx.clone();
        let allocator = self.allocator.clone();
        let last_emit = self.last_emit.clone();
        let denials = self.denials.clone();
        Box::new(move |name: &str, ok: bool, output: &str| {
            let status = if ok {
                ToolCallStatus::Completed
            } else {
                ToolCallStatus::Failed
            };
            // Consume the gate's own record rather than searching `output` for a marker:
            // `output` is model-influenceable, this is not (D-12, `DenialLedger`).
            let denied = !ok && denials.take(name);
            let content =
                crate::tool_render::render_result_content(name, output.as_bytes(), ok, denied);
            allocator.finish(name, &tx, &last_emit, status, content);
        })
    }

    /// Enqueues an arbitrary `SessionUpdate` through the same channel the three typed
    /// callbacks use — for updates that don't originate from a per-turn callback (e.g. the
    /// post-`run_turn` usage update, plan 03 task 2) but must still preserve ordering
    /// relative to whatever is still draining.
    pub fn send_update(&self, update: SessionUpdate) {
        enqueue(&self.tx, &self.last_emit, update);
    }

    /// The session id this bridge notifies. Exposed for callers that want to confirm
    /// wiring without re-deriving it.
    pub fn session_id(&self) -> &SessionId {
        &self.session_id
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status_of(update: &SessionUpdate) -> (String, Option<ToolCallStatus>) {
        match update {
            SessionUpdate::ToolCall(call) => {
                (format!("{:?}", call.tool_call_id), Some(call.status))
            }
            SessionUpdate::ToolCallUpdate(u) => (format!("{:?}", u.tool_call_id), u.fields.status),
            other => (format!("{other:?}"), None),
        }
    }

    /// WR-02 regression: a keepalive heartbeat must never be emitted for a tool call whose
    /// terminal update has already gone out.
    ///
    /// The ticker task and the agent-loop task touch the same allocator from different
    /// threads. When the in-flight lookup and the send are separate lock acquisitions, the
    /// ticker can read an id, get preempted while the agent loop sends that call's terminal
    /// update and clears the entry, and then send `InProgress` for it. Both go down one
    /// channel, so the client sees `Completed` followed by `InProgress` — and since the
    /// entry is now cleared, nothing further is ever sent for that id, leaving an editor
    /// rendering a permanent spinner on a finished call.
    ///
    /// This drives the real interleaving rather than simulating it. A hammer thread runs
    /// the heartbeat path while the main thread finishes the call; a `Barrier` releases the
    /// two at the same instant each round so the heartbeats actually overlap `finish`
    /// rather than all landing before it (without the barrier the hammer thread is still
    /// starting up when the round is already over, and the test passes against known-broken
    /// code). A third thread drains the channel as it fills and checks ordering in a single
    /// streaming pass, which is also what keeps the unbounded channel from growing.
    ///
    /// The assertion is the invariant that matters to a client: for any one
    /// `tool_call_id`, nothing follows its terminal status. It cannot fail spuriously —
    /// only a genuinely non-atomic read-then-send can produce the bad order. Verified to
    /// fail against the non-atomic implementation this replaced.
    #[test]
    fn heartbeat_never_lands_after_a_terminal_update_for_the_same_call() {
        const ROUNDS: usize = 20_000;
        const HEARTBEATS_PER_ROUND: usize = 8;

        let (tx, mut rx) = mpsc::unbounded_channel::<SessionUpdate>();
        let last_emit = Arc::new(Mutex::new(Instant::now()));
        let allocator = Arc::new(ToolCallIdAllocator::new());
        let barrier = Arc::new(std::sync::Barrier::new(2));

        let checker = std::thread::spawn(move || {
            let mut terminated: std::collections::HashSet<String> = std::collections::HashSet::new();
            let mut seen_terminal = 0usize;
            while let Some(update) = rx.blocking_recv() {
                let (id, status) = status_of(&update);
                assert!(
                    !terminated.contains(&id),
                    "{update:?} was emitted for a tool call AFTER its terminal update — a \
                     client tracking this call is now stuck rendering it as still running, \
                     because nothing further is ever sent for a cleared id"
                );
                if matches!(status, Some(ToolCallStatus::Completed | ToolCallStatus::Failed)) {
                    terminated.insert(id);
                    seen_terminal += 1;
                }
            }
            seen_terminal
        });

        let hammer = {
            let allocator = allocator.clone();
            let tx = tx.clone();
            let last_emit = last_emit.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                for _ in 0..ROUNDS {
                    barrier.wait();
                    for _ in 0..HEARTBEATS_PER_ROUND {
                        allocator.heartbeat_if_in_flight(&tx, &last_emit);
                    }
                }
            })
        };

        for _ in 0..ROUNDS {
            // Register the call BEFORE the barrier, so the hammer has something in flight
            // to heartbeat on the moment it is released.
            allocator.progress("terminal", &tx, &last_emit, Vec::new());
            barrier.wait();
            allocator.finish(
                "terminal",
                &tx,
                &last_emit,
                ToolCallStatus::Completed,
                Vec::new(),
            );
        }

        hammer.join().expect("hammer thread should not panic");
        drop(tx);
        let seen_terminal = checker.join().expect("ordering invariant violated");
        assert_eq!(
            seen_terminal, ROUNDS,
            "every round must have produced exactly one terminal update"
        );
    }
}
