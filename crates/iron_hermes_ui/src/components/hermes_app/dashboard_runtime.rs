//! Phase 54.10 Plan 09 (D-05, D-06): the Dashboard's generation runtime.
//!
//! A board holds many AI widgets and a turn must outlive navigation away from the Dashboard, so
//! this is a root-owned sibling of `use_openui_runtime` (RESEARCH Pattern 4, add-alongside): its
//! own lazily connected socket and its own chat session, never `OpenUiCtx` or `OPENUI_FRAME_ID`.
//! Jobs run strictly one at a time. A job exists only because an operator pressed a labelled
//! control (Re-run, Retry widget, an action button inside a widget); nothing here ever starts a
//! turn from a clock, poll or effect (D-06).
//!
//! Every turn is a normal agent turn on the dashboard session, so tools, guardrails and
//! approvals apply unchanged; approval events land in the root approval map and the card shows
//! in Chat. Turns never use the Thesys hosted Gateway (`openui_gateway: false`).

use crate::components::hermes_app::screens::openui::{
    FrameCmd, OpenUiDoc, OpenUiStream, RerunOutcome, openui_request, post_to_frame_id,
    rerun_outcome,
};
use crate::protocol::ChatStreamEvent;
use crate::server::openui_views_api::replace_openui_snapshot;
use dioxus::prelude::*;
use std::collections::{HashMap, HashSet, VecDeque};

/// What started a job. Plans 54.10-10 and 54.10-12 add variants.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum GenKind {
    /// Re-run a widget's saved prompt; a clean finish replaces the saved snapshot (D-06).
    Rerun,
    /// A button inside a widget; the snapshot is left alone (54.9 parity).
    Action,
    /// The one Plan with AI turn (D-15): a plain agent turn (`openui: false`) whose final text is
    /// handed to the Plan panel as data. It belongs to no widget and saves nothing.
    Plan,
}

/// The queue key of the planning turn (it has no widget).
pub(crate) const PLAN_ITEM_ID: &str = "__plan__";

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct GenJob {
    pub item_id: String,
    pub view_id: String,
    pub message: String,
    pub kind: GenKind,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum GenStatus {
    Queued,
    Generating,
    Done,
    Error(String),
    Stopped,
}

/// Where a cancelled item's job was.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Cancelled {
    Running,
    Queued,
    Nothing,
}

/// FIFO queue with at most one running job.
#[derive(Default)]
pub(crate) struct GenQueue {
    queue: VecDeque<GenJob>,
    current: Option<GenJob>,
}

impl GenQueue {
    /// Queue a job. One job per widget: a second one for an item that already has a queued or
    /// running job is refused (the buttons of a half-streamed view are not final).
    pub(crate) fn enqueue(&mut self, job: GenJob) -> bool {
        let busy = self
            .current
            .as_ref()
            .is_some_and(|c| c.item_id == job.item_id)
            || self.queue.iter().any(|q| q.item_id == job.item_id);
        if busy {
            return false;
        }
        self.queue.push_back(job);
        true
    }

    /// Take the next job to run, or `None` while one is running or nothing is queued.
    pub(crate) fn start_next(&mut self) -> Option<GenJob> {
        if self.current.is_some() {
            return None;
        }
        self.current = self.queue.pop_front();
        self.current.clone()
    }

    pub(crate) fn current(&self) -> Option<&GenJob> {
        self.current.as_ref()
    }

    /// The running job ended; returns it when it was `item_id`'s.
    pub(crate) fn finish(&mut self, item_id: &str) -> Option<GenJob> {
        if self.current.as_ref().is_some_and(|c| c.item_id == item_id) {
            return self.current.take();
        }
        None
    }

    /// Drop a queued job. A running job stays until its turn ends (the caller sends `/stop`).
    pub(crate) fn cancel(&mut self, item_id: &str) -> Cancelled {
        if self.current.as_ref().is_some_and(|c| c.item_id == item_id) {
            return Cancelled::Running;
        }
        let before = self.queue.len();
        self.queue.retain(|q| q.item_id != item_id);
        if self.queue.len() < before {
            Cancelled::Queued
        } else {
            Cancelled::Nothing
        }
    }
}

/// What a finished job does to the saved snapshot: only a clean Re-run with text replaces it.
pub(crate) fn finish_outcome(kind: GenKind, finished_ok: bool, text: &str) -> RerunOutcome {
    match kind {
        GenKind::Rerun => rerun_outcome(finished_ok, text),
        GenKind::Action | GenKind::Plan => RerunOutcome::Keep,
    }
}

/// The chat request of a job. A Plan turn is a normal chat turn (`openui: false`); the others are
/// OpenUI turns on own providers only (the hosted Gateway is the OpenUI screen's explicit toggle).
fn job_request(session_id: &str, job: &GenJob) -> crate::protocol::ChatRequest {
    match job.kind {
        GenKind::Plan => crate::protocol::ChatRequest {
            session_id: session_id.to_string(),
            message: job.message.clone(),
            active_identity: None,
            attachment_ids: Vec::new(),
            openui: false,
            openui_gateway: false,
            openui_prompt: false,
        },
        kind => openui_request(
            session_id,
            job.message.clone(),
            false,
            kind == GenKind::Rerun,
        ),
    }
}

/// Folds the chat stream into frame commands for ONE job. A fresh one is made when the job
/// starts. It is armed only by that job's own `TurnStarted` and then follows that turn id alone,
/// so the tail of an earlier (stopped) job and the nil-id reply of a `/stop` slash command
/// (`Delta` + `Finished` with a nil turn id) never reach it.
#[derive(Default)]
struct JobStream {
    inner: OpenUiStream,
    armed: bool,
}

impl JobStream {
    /// `stopping`: the operator pressed Stop on this job, so the `/stop` reply's nil-id
    /// `Finished` is the one nil-id event that ends it (its own turn may never report).
    fn apply(&mut self, ev: &ChatStreamEvent, stopping: bool) -> Vec<FrameCmd> {
        use ChatStreamEvent as E;
        match ev {
            E::TurnStarted { turn_id, .. } if !self.armed && !turn_id.is_nil() => {
                self.armed = true;
                self.inner.apply(ev)
            }
            E::TurnStarted { .. } => Vec::new(),
            // A nil-id Error is a rejected request (ws.rs); it can fail the job before it starts.
            E::Error { turn_id, .. } if turn_id.is_nil() || self.armed => self.inner.apply(ev),
            E::Finished { turn_id, .. } if turn_id.is_nil() => {
                if stopping {
                    let end = E::TurnCancelled {
                        turn_id: uuid::Uuid::nil(),
                    };
                    self.inner.apply(&end)
                } else {
                    Vec::new()
                }
            }
            E::Delta { turn_id, .. }
            | E::ToolCallEnd { turn_id, .. }
            | E::TurnCancelled { turn_id }
            | E::TurnEnded { turn_id, .. }
            | E::Finished { turn_id, .. }
            | E::Error { turn_id, .. }
                if !self.armed || turn_id.is_nil() =>
            {
                Vec::new()
            }
            _ => self.inner.apply(ev),
        }
    }
}

/// Root-provided handles to the dashboard runtime. Everything is a `Copy` signal or callback.
#[derive(Clone, Copy)]
pub(crate) struct DashboardGenCtx {
    /// Queue a job; `false` when that widget already has one.
    pub enqueue: Callback<GenJob, bool>,
    /// Stop a running job (sends `/stop`) or drop a queued one.
    pub stop: Callback<String>,
    /// The item is gone (widget removed, board deleted): stop its queued or running job and forget
    /// its per-item state, so no turn is spent on a view nobody can see.
    pub cancel_item: Callback<String>,
    pub statuses: Signal<HashMap<String, GenStatus>>,
    pub docs: Signal<HashMap<String, OpenUiDoc>>,
    /// Item ids whose frame has loaded (posts only go to these).
    pub loaded_frames: Signal<HashSet<String>>,
    /// The label of the action last sent from each widget.
    pub action_labels: Signal<HashMap<String, String>>,
    /// Bumped after a snapshot is replaced; the Dashboard refetches its saved-view list.
    pub views_rev: Signal<u32>,
    /// The finished planning turn: its text, or why it failed. The Plan panel takes it.
    pub plan_result: Signal<Option<Result<String, String>>>,
}

#[derive(Clone, Copy)]
struct Rt {
    plan_result: Signal<Option<Result<String, String>>>,
    queue: Signal<GenQueue>,
    statuses: Signal<HashMap<String, GenStatus>>,
    docs: Signal<HashMap<String, OpenUiDoc>>,
    loaded_frames: Signal<HashSet<String>>,
    session_id: Signal<String>,
    /// The active-profile tick (`TopbarProfileRefreshCtx`) now, and when `session_id` was made.
    profile_tick: Signal<u32>,
    session_tick: Signal<u32>,
    send: Callback<String>,
    stream: Signal<JobStream>,
}

/// A session made under another active profile must not carry that profile's prompts, outputs
/// and tool results into this profile's turns, so it is replaced by a fresh one.
fn session_stale(session_id: &str, made_at_tick: u32, tick: u32) -> bool {
    session_id.is_empty() || made_at_tick != tick
}

fn set_status(mut statuses: Signal<HashMap<String, GenStatus>>, item: &str, s: GenStatus) {
    statuses.with_mut(|m| {
        m.insert(item.to_string(), s);
    });
}

/// A stop request on the dashboard session.
fn stop_request(session_id: &str) -> crate::protocol::ChatRequest {
    crate::protocol::ChatRequest {
        session_id: session_id.to_string(),
        message: "/stop".to_string(),
        active_identity: None,
        attachment_ids: Vec::new(),
        openui: false,
        openui_gateway: false,
        openui_prompt: false,
    }
}

/// Start the next queued job, if none is running.
fn pump(mut rt: Rt) {
    let Some(job) = rt.queue.write_unchecked().start_next() else {
        return;
    };
    set_status(rt.statuses, &job.item_id, GenStatus::Generating);
    rt.stream.set(JobStream::default());
    rt.docs.with_mut(|m| {
        m.insert(job.item_id.clone(), OpenUiDoc::default());
    });
    if rt.loaded_frames.peek().contains(&job.item_id) {
        post_to_frame_id(
            &super::screens::dashboard::widget_frame_id(&job.item_id),
            &FrameCmd::Reset,
        );
    }
    spawn(async move {
        let mut session_id = rt.session_id;
        let tick = *rt.profile_tick.peek();
        let sid = if session_stale(&session_id.peek(), *rt.session_tick.peek(), tick) {
            match crate::server::api::create_session().await {
                Ok(new) => {
                    session_id.set(new.clone());
                    let mut session_tick = rt.session_tick;
                    session_tick.set(tick);
                    Some(new)
                }
                Err(_) => None,
            }
        } else {
            Some(session_id.peek().clone())
        };
        // Stop pressed while the session was being created: the job never went out, so drop it.
        if matches!(
            rt.statuses.peek().get(&job.item_id),
            Some(GenStatus::Stopped)
        ) {
            rt.queue.write_unchecked().finish(&job.item_id);
            pump(rt);
            return;
        }
        let sent = sid.and_then(|sid| serde_json::to_string(&job_request(&sid, &job)).ok());
        match sent {
            Some(json) => rt.send.call(json),
            None => {
                fail_job(rt, &job, "the session is not ready yet");
                rt.queue.write_unchecked().finish(&job.item_id);
                pump(rt);
            }
        }
    });
}

/// Called ONCE from `HermesApp`, right after `use_openui_runtime`, unconditionally (hook order).
pub(crate) fn use_dashboard_runtime(
    mut approval_cards: Signal<
        crate::components::hermes_app::screens::chat::approval_card::ApprovalCards,
    >,
    mut bubbles: Signal<Vec<crate::components::hermes_app::screens::chat::ChatBubble>>,
    mut next_id: Signal<u64>,
) {
    use crate::components::hermes_app::screens::chat::approval_card::{
        apply_approval_event, apply_disconnect_for_session,
    };

    let queue = use_signal(GenQueue::default);
    let statuses = use_signal(HashMap::<String, GenStatus>::new);
    let docs = use_signal(HashMap::<String, OpenUiDoc>::new);
    let loaded_frames = use_signal(HashSet::<String>::new);
    let action_labels = use_signal(HashMap::<String, String>::new);
    let views_rev = use_signal(|| 0u32);
    let plan_result = use_signal(|| None::<Result<String, String>>);
    let session_id = use_signal(String::new);
    let mut connect = use_signal(|| false);

    // Lazy connect: the socket opens on the first enqueue, never for an operator who does not
    // use AI widgets. Until `connect` flips the future never resolves.
    let mut ws = dioxus_fullstack::use_websocket(move || {
        let wanted = *connect.read();
        async move {
            if !wanted {
                std::future::pending::<()>().await;
            }
            crate::server::ws::ws_chat(
                dioxus_fullstack::WebSocketOptions::new().with_automatic_reconnect(),
            )
            .await
        }
    });

    // A lost send would leave the running job `Generating` forever and block the queue, so a
    // failed send bumps this and the effect below fails the running job and moves on.
    let mut send_failed = use_signal(|| 0u32);
    let send = use_callback(move |json: String| {
        spawn(async move {
            if ws
                .send_raw(dioxus_fullstack::Message::Text(json))
                .await
                .is_err()
            {
                send_failed += 1;
            }
        });
    });
    let rt = Rt {
        plan_result,
        queue,
        statuses,
        docs,
        loaded_frames,
        session_id,
        profile_tick: use_context::<
            crate::components::hermes_app::profile_topbar::TopbarProfileRefreshCtx,
        >()
        .0,
        session_tick: use_signal(|| 0u32),
        send,
        stream: use_signal(JobStream::default),
    };

    use_effect(move || {
        if send_failed() == 0 {
            return;
        }
        let running = rt.queue.peek().current().cloned();
        if let Some(job) = running {
            fail_job(rt, &job, "could not reach the server");
            rt.queue.write_unchecked().finish(&job.item_id);
            pump(rt);
        }
    });

    // Receive loop: no signal borrow is held across an `.await`.
    use_future(move || async move {
        loop {
            let _state = ws.connect().await;
            if ws.is_err() {
                continue;
            }
            // The socket is open: start whatever queued up while it was down (no-op otherwise).
            pump(rt);
            loop {
                match ws.recv_raw().await {
                    Ok(dioxus_fullstack::Message::Text(t)) => {
                        let Ok(event) = serde_json::from_str::<ChatStreamEvent>(&t) else {
                            continue;
                        };
                        let job = rt.queue.peek().current().cloned();
                        let stopping = job.as_ref().is_some_and(|j| {
                            matches!(rt.statuses.peek().get(&j.item_id), Some(GenStatus::Stopped))
                        });
                        let cmds = rt.stream.write_unchecked().apply(&event, stopping);
                        for cmd in cmds {
                            let Some(job) = job.as_ref() else {
                                continue;
                            };
                            let item = &job.item_id;
                            rt.docs
                                .write_unchecked()
                                .entry(item.clone())
                                .or_default()
                                .apply(&cmd);
                            if rt.loaded_frames.peek().contains(item) {
                                post_to_frame_id(
                                    &super::screens::dashboard::widget_frame_id(item),
                                    &cmd,
                                );
                            }
                            if cmd == FrameCmd::End {
                                finish_job(rt, job, &event, views_rev);
                            }
                        }
                        if let ChatStreamEvent::ApprovalRequested { .. }
                        | ChatStreamEvent::ApprovalResolved { .. } = &event
                        {
                            // The card lives in Chat and the badge on the Chat wedge: apply the
                            // event to the root map exactly as the OpenUI runtime does.
                            let mut local_cards = approval_cards.peek().clone();
                            let mut local_next = *next_id.peek();
                            let changed =
                                if matches!(event, ChatStreamEvent::ApprovalRequested { .. }) {
                                    apply_approval_event(
                                        &mut bubbles.write(),
                                        &mut local_next,
                                        &mut local_cards,
                                        &event,
                                    )
                                } else {
                                    apply_approval_event(
                                        &mut Vec::new(),
                                        &mut local_next,
                                        &mut local_cards,
                                        &event,
                                    )
                                };
                            if local_next != *next_id.peek() {
                                next_id.set(local_next);
                            }
                            if changed {
                                approval_cards.set(local_cards);
                            }
                        }
                    }
                    Ok(dioxus_fullstack::Message::Close { .. }) | Err(_) => {
                        // Only the cards this socket's own session raised; Chat's stay. The
                        // server cancels a turn whose socket closes, so a running job failed.
                        let own = rt.session_id.peek().clone();
                        let mut local_cards = approval_cards.peek().clone();
                        if apply_disconnect_for_session(&mut local_cards, &own) {
                            approval_cards.set(local_cards);
                        }
                        let running = rt.queue.peek().current().cloned();
                        if let Some(job) = running {
                            fail_job(rt, &job, "the connection closed");
                            // The next job starts once the socket reports open again.
                            rt.queue.write_unchecked().finish(&job.item_id);
                        }
                        break;
                    }
                    _ => continue,
                }
            }
        }
    });

    let enqueue = use_callback(move |job: GenJob| {
        let item = job.item_id.clone();
        let accepted = rt.queue.write_unchecked().enqueue(job);
        if accepted {
            set_status(rt.statuses, &item, GenStatus::Queued);
            if !*connect.peek() {
                connect.set(true);
            }
            pump(rt);
        }
        accepted
    });

    let stop = use_callback(move |item: String| {
        let cancelled = rt.queue.write_unchecked().cancel(&item);
        match cancelled {
            Cancelled::Running => {
                set_status(rt.statuses, &item, GenStatus::Stopped);
                // No session yet means the job's request was never sent: pump drops it.
                let sid = rt.session_id.peek().clone();
                if !sid.is_empty() {
                    if let Ok(json) = serde_json::to_string(&stop_request(&sid)) {
                        rt.send.call(json);
                    }
                }
            }
            Cancelled::Queued => {
                let mut statuses = rt.statuses;
                statuses.with_mut(|m| {
                    m.remove(&item);
                });
            }
            Cancelled::Nothing => {}
        }
    });

    let cancel_item = use_callback(move |item: String| {
        stop.call(item.clone());
        // A running job still needs its status and doc until its turn ends (`finish_job`).
        if rt.queue.peek().current().is_some_and(|c| c.item_id == item) {
            return;
        }
        let (mut statuses, mut docs, mut labels) = (rt.statuses, rt.docs, action_labels);
        statuses.with_mut(|m| {
            m.remove(&item);
        });
        docs.with_mut(|m| {
            m.remove(&item);
        });
        labels.with_mut(|m| {
            m.remove(&item);
        });
    });

    use_context_provider(|| DashboardGenCtx {
        enqueue,
        stop,
        cancel_item,
        statuses,
        docs,
        loaded_frames,
        action_labels,
        views_rev,
        plan_result,
    });
}

/// The running job's turn ended: settle its status, replace the snapshot when that is due, and
/// start the next job.
/// A job that failed before or without a terminal stream event. A planning turn also publishes
/// the failure, so the panel shows the failure copy instead of silently dropping back to the brief.
fn fail_job(rt: Rt, job: &GenJob, message: &str) {
    if job.kind == GenKind::Plan {
        let mut plan_result = rt.plan_result;
        plan_result.set(Some(Err(message.to_string())));
    }
    set_status(
        rt.statuses,
        &job.item_id,
        GenStatus::Error(message.to_string()),
    );
}

fn finish_job(rt: Rt, job: &GenJob, event: &ChatStreamEvent, mut views_rev: Signal<u32>) {
    let ok = matches!(
        event,
        ChatStreamEvent::Finished { .. } | ChatStreamEvent::TurnEnded { .. }
    );
    let stopped = matches!(
        rt.statuses.peek().get(&job.item_id),
        Some(GenStatus::Stopped)
    );
    let status = match event {
        ChatStreamEvent::Error { message, .. } => GenStatus::Error(message.clone()),
        _ if stopped || !ok => GenStatus::Stopped,
        _ => GenStatus::Done,
    };
    if job.kind == GenKind::Plan {
        // A stopped plan hands nothing over; the panel falls back to the brief.
        let mut plan_result = rt.plan_result;
        match &status {
            GenStatus::Done => {
                let text = rt
                    .docs
                    .peek()
                    .get(&job.item_id)
                    .map(|d| d.text().to_string())
                    .unwrap_or_default();
                plan_result.set(Some(Ok(text)));
            }
            GenStatus::Error(m) => plan_result.set(Some(Err(m.clone()))),
            _ => {}
        }
    }
    set_status(rt.statuses, &job.item_id, status);
    let text = rt
        .docs
        .peek()
        .get(&job.item_id)
        .map(|d| d.text().to_string())
        .unwrap_or_default();
    if let RerunOutcome::Replace(text) = finish_outcome(job.kind, ok && !stopped, &text) {
        let (view_id, item_id) = (job.view_id.clone(), job.item_id.clone());
        let statuses = rt.statuses;
        spawn(async move {
            // A rejected or failed write must not read as Done: the result would revert on reload.
            match replace_openui_snapshot(view_id, text).await {
                Ok(_) => views_rev.with_mut(|r| *r += 1),
                // Only while the widget still shows Done: a newer job's status wins.
                Err(e) if statuses.peek().get(&item_id) == Some(&GenStatus::Done) => set_status(
                    statuses,
                    &item_id,
                    GenStatus::Error(format!("Could not save the result: {e}")),
                ),
                Err(_) => {}
            }
        });
    }
    rt.queue.write_unchecked().finish(&job.item_id);
    pump(rt);
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn ev_delta(id: Uuid) -> ChatStreamEvent {
        ChatStreamEvent::Delta {
            turn_id: id,
            text: "x".into(),
        }
    }
    fn ev_finished(id: Uuid) -> ChatStreamEvent {
        ChatStreamEvent::Finished {
            turn_id: id,
            total_tokens: 0,
        }
    }
    fn ev_started(id: Uuid) -> ChatStreamEvent {
        ChatStreamEvent::TurnStarted {
            turn_id: id,
            session_id: "s".into(),
            index: 0,
        }
    }

    #[test]
    fn stop_reply_never_reaches_the_next_job() {
        let (b, nil) = (Uuid::new_v4(), Uuid::nil());
        // Job B has just started: the /stop reply of job A and A's late tail arrive first.
        let mut s = JobStream::default();
        assert!(s.apply(&ev_delta(nil), false).is_empty());
        assert!(s.apply(&ev_finished(nil), false).is_empty());
        assert!(
            s.apply(&ev_delta(Uuid::new_v4()), false).is_empty(),
            "A's tail"
        );
        let cancelled = ChatStreamEvent::TurnCancelled {
            turn_id: Uuid::new_v4(),
        };
        assert!(s.apply(&cancelled, false).is_empty());
        // B's own turn arms the stream and ends it; a nil-id reply mid-turn is still ignored.
        assert!(s.apply(&ev_started(b), false).is_empty());
        assert_eq!(
            s.apply(&ev_delta(b), false),
            vec![FrameCmd::Push("x".into())]
        );
        assert!(s.apply(&ev_delta(nil), false).is_empty());
        assert!(s.apply(&ev_finished(nil), false).is_empty());
        assert!(s.apply(&ev_delta(Uuid::new_v4()), false).is_empty());
        assert_eq!(s.apply(&ev_finished(b), false), vec![FrameCmd::End]);
    }

    #[test]
    fn a_session_is_replaced_when_the_active_profile_changes() {
        assert!(session_stale("", 0, 0), "no session yet");
        assert!(!session_stale("s", 2, 2));
        assert!(
            session_stale("s", 2, 3),
            "profile switched since it was made"
        );
    }

    #[test]
    fn stop_reply_ends_only_the_stopped_job() {
        let nil = Uuid::nil();
        let mut s = JobStream::default();
        s.apply(&ev_started(Uuid::new_v4()), false);
        assert_eq!(s.apply(&ev_finished(nil), true), vec![FrameCmd::End]);
        assert!(s.apply(&ev_finished(nil), true).is_empty(), "only one End");
        // A rejected request (nil-id Error) can still fail an unarmed job.
        let mut s = JobStream::default();
        let err = ChatStreamEvent::Error {
            turn_id: nil,
            message: "no".into(),
        };
        assert_eq!(s.apply(&err, false), vec![FrameCmd::End]);
    }

    fn job(item: &str, kind: GenKind) -> GenJob {
        GenJob {
            item_id: item.into(),
            view_id: format!("view-{item}"),
            message: "go".into(),
            kind,
        }
    }

    #[test]
    fn gen_queue_runs_one_job_at_a_time() {
        let mut q = GenQueue::default();
        assert!(q.enqueue(job("a", GenKind::Rerun)));
        assert!(q.enqueue(job("b", GenKind::Rerun)));
        assert_eq!(q.start_next().unwrap().item_id, "a");
        assert!(q.start_next().is_none(), "A is still running");
        assert!(q.finish("b").is_none(), "only the running job can finish");
        assert_eq!(q.finish("a").unwrap().item_id, "a");
        assert_eq!(q.start_next().unwrap().item_id, "b");
        // One job per widget, running or queued.
        assert!(!q.enqueue(job("b", GenKind::Action)));
        assert!(q.enqueue(job("c", GenKind::Action)));
        assert!(!q.enqueue(job("c", GenKind::Action)));
    }

    #[test]
    fn gen_queue_only_enqueue_starts_turns() {
        let mut q = GenQueue::default();
        assert!(q.start_next().is_none(), "a fresh queue yields nothing");
        assert!(q.current().is_none());
        q.enqueue(job("a", GenKind::Rerun));
        q.enqueue(job("b", GenKind::Rerun));
        assert_eq!(q.cancel("b"), Cancelled::Queued);
        assert_eq!(q.cancel("b"), Cancelled::Nothing);
        q.start_next();
        assert_eq!(q.cancel("a"), Cancelled::Running);
        assert_eq!(q.finish("a").unwrap().item_id, "a");
        assert!(q.start_next().is_none(), "the cancelled job never runs");
    }

    #[test]
    fn rerun_finish_replaces_only_on_clean_text() {
        assert_eq!(
            finish_outcome(GenKind::Rerun, true, "<ui/>"),
            RerunOutcome::Replace("<ui/>".into())
        );
        assert_eq!(
            finish_outcome(GenKind::Rerun, false, "<ui/>"),
            RerunOutcome::Keep
        );
        assert_eq!(
            finish_outcome(GenKind::Rerun, true, "  "),
            RerunOutcome::Keep
        );
    }

    #[test]
    fn action_job_does_not_replace_snapshot() {
        assert_eq!(
            finish_outcome(GenKind::Action, true, "<ui/>"),
            RerunOutcome::Keep
        );
    }

    #[test]
    fn plan_job_is_a_plain_chat_turn() {
        let plan = job_request("s", &job(PLAN_ITEM_ID, GenKind::Plan));
        assert!(!plan.openui && !plan.openui_gateway && !plan.openui_prompt);
        let rerun = job_request("s", &job("a", GenKind::Rerun));
        assert!(rerun.openui && rerun.openui_prompt && !rerun.openui_gateway);
        let action = job_request("s", &job("a", GenKind::Action));
        assert!(action.openui && !action.openui_prompt);
        assert_eq!(
            finish_outcome(GenKind::Plan, true, "{}"),
            RerunOutcome::Keep,
            "a plan never touches a saved snapshot"
        );
    }

    #[test]
    fn dashboard_runtime_has_no_timer() {
        const SRC: &str = include_str!("dashboard_runtime.rs");
        let code = &SRC[..SRC.find(concat!("#[cfg(", "test)]")).expect("test module")];
        let needle = concat!("timer::", "sleep");
        assert!(!code.contains(needle), "no clock may start a generation");
        assert!(!code.contains(concat!("use_", "interval")));
    }
}
