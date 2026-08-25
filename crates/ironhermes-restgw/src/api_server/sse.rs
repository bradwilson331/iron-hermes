//! `StreamEvent` -> SSE frame bridge — Task 3 of Phase 36.7.1 Plan 06.
//!
//! Maps FROM [`ironhermes_agent::client::StreamEvent`] — the five-variant type the
//! agent loop actually produces — into one SSE frame type per source variant, and
//! nothing else (API-11, T-36.7.1-51). This repository contains three different
//! `StreamEvent`-shaped enums; the terminal UI's eight-variant type and the web UI's
//! protocol type belong to those surfaces, not this one, and importing either here
//! would be a cross-tier dependency pointing the wrong way. No variant is added to
//! any of the three.
//!
//! Also owns [`RunEventRegistry`] — the run-id-keyed channel bridge between whatever
//! spawns and drives a submitted run ([`super::routes::runs`]) and the streaming
//! route that serves it out as SSE.

use std::collections::HashMap;
use std::convert::Infallible;
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::response::sse::Event;
use ironhermes_agent::client::StreamEvent;
use ironhermes_core::concurrency::TurnId;
use serde::Serialize;
use tokio::sync::{Mutex, mpsc};
use tokio_stream::Stream;
use tokio_stream::wrappers::UnboundedReceiverStream;
use tokio_util::sync::CancellationToken;

use crate::webhook::idempotency::{Clock, SystemClock};

/// How long a [`RunEventRegistry`] entry is retained before it is pruned
/// (code review WR-01). Generously longer than any run is expected to take,
/// because expiring a run whose producer is still emitting would turn a slow
/// answer into a lost stream — the TTL exists to bound a leak, not to time
/// runs out. Same value and same rationale as the webhook adapter's
/// `ORIGIN_CALLBACK_TTL`.
const RUN_EVENT_TTL: Duration = Duration::from_secs(3600);

/// Hard cap on retained entries in either [`RunEventRegistry`] map, covering
/// the case the TTL cannot: sustained arrival faster than entries expire.
pub const RUN_EVENT_MAX_ENTRIES: usize = 10_000;

/// One SSE frame type per [`StreamEvent`] variant — a fixed set, not an open one.
/// `sse_frames_map_only_known_variants` asserts every frame emitted by
/// [`build_stream`] carries a `type` drawn from exactly this set.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SseFrame {
    ContentDelta {
        text: String,
    },
    ToolCallDelta {
        index: usize,
        #[serde(skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        arguments: Option<String>,
    },
    Usage {
        usage: ironhermes_core::Usage,
    },
    Done {
        #[serde(skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
    ProviderError {
        message: String,
    },
}

impl From<&StreamEvent> for SseFrame {
    fn from(event: &StreamEvent) -> Self {
        match event {
            StreamEvent::ContentDelta(text) => SseFrame::ContentDelta { text: text.clone() },
            StreamEvent::ToolCallDelta {
                index,
                id,
                name,
                arguments,
            } => SseFrame::ToolCallDelta {
                index: *index,
                id: id.clone(),
                name: name.clone(),
                arguments: arguments.clone(),
            },
            StreamEvent::Usage(usage) => SseFrame::Usage {
                usage: usage.clone(),
            },
            StreamEvent::Done(reason) => SseFrame::Done {
                reason: reason.clone(),
            },
            StreamEvent::ProviderError(message) => SseFrame::ProviderError {
                message: message.clone(),
            },
        }
    }
}

/// Render one [`SseFrame`] as an axum SSE [`Event`] — JSON body, no hand-written
/// frame formatting (the plan's own instruction: reach for axum's SSE type rather
/// than formatting the wire form by hand).
fn frame_to_event(frame: &SseFrame) -> Event {
    match serde_json::to_string(frame) {
        Ok(body) => Event::default().data(body),
        Err(_) => Event::default().data("{}"),
    }
}

/// Build the SSE byte stream for one run's event channel.
///
/// Forwards every [`StreamEvent`] received on `rx` as one [`SseFrame`], and
/// terminates when EITHER: `rx` closes (the producer finished — normal completion,
/// after sending a `Done`/`ProviderError` frame and dropping its sender), OR
/// `cancel` fires (a `POST /v1/runs/{id}/stop` mid-stream) — whichever happens
/// first. A stopped run's stream therefore closes even if its producer has not yet
/// noticed the cancellation and dropped its sender (`cancelled_run_terminates_its_stream`).
///
/// Implemented as a forwarding task rather than a `StreamExt` combinator — this
/// crate does not carry a direct `futures`/`futures-util` dependency, and a small
/// `tokio::select!` loop over `mpsc` channels needs only what is already a direct
/// dependency here.
pub fn build_stream(
    mut rx: mpsc::UnboundedReceiver<StreamEvent>,
    cancel: CancellationToken,
) -> impl Stream<Item = Result<Event, Infallible>> + Send + 'static {
    let (out_tx, out_rx) = mpsc::unbounded_channel::<Result<Event, Infallible>>();
    tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = cancel.cancelled() => break,
                maybe_event = rx.recv() => {
                    match maybe_event {
                        Some(event) => {
                            let frame = frame_to_event(&SseFrame::from(&event));
                            if out_tx.send(Ok(frame)).is_err() {
                                break; // client disconnected — output receiver dropped.
                            }
                        }
                        None => break, // producer finished (channel closed after Done/ProviderError).
                    }
                }
            }
        }
    });
    UnboundedReceiverStream::new(out_rx)
}

/// Bridges a submitted run's producer to the SSE route that streams it out.
///
/// The producer calls [`RunEventRegistry::register`] BEFORE it starts emitting
/// events, obtaining the sending half of a fresh channel; [`RunEventRegistry::take`]
/// hands the receiving half to the first (and only) client that opens
/// `GET /v1/runs/{run_id}/events` for that run. A second open for the same run finds
/// nothing (single-subscriber) and is reported not-found, same as an unknown run.
///
/// Also tracks which run identifiers have completed (Task 2's status route), so a
/// finished run can be reported distinctly from one that never existed rather than
/// collapsing both into the same not-found response.
///
/// Deliberately NOT part of [`super::ApiServerHandles`] — see that field's own doc
/// comment for why: unlike the approval gate, this has no cross-crate type to bridge.
///
/// **Both maps are bounded** (code review WR-01). Before that, `pending` was
/// drained only by [`Self::take`] — reached solely from
/// `GET /v1/runs/{id}/events` — and `completed` was never drained at all;
/// neither was pruned, capped or TTL'd. That is not an edge case, it is the
/// DEFAULT outcome: `run_stub_turn` finishes in ~150 ms and the `events` route
/// refuses with 404 unless the turn registry still holds the entry, so a
/// client that submits and opens the stream a moment later gets its 404 AND
/// leaves the receiver in `pending` forever, holding the whole echoed prompt.
/// `POST /v1/runs` with a 1 MiB prompt, repeated, therefore retained 1 MiB per
/// submission plus one `TurnId` for the process lifetime.
///
/// Same defect class as the `origin_callbacks` table (security audit N-02) on
/// a second surface, and given the same treatment: entries carry an insertion
/// instant, are pruned lazily on access against [`RUN_EVENT_TTL`], and are
/// backstopped by [`RUN_EVENT_MAX_ENTRIES`] for the case the TTL cannot cover
/// (sustained arrival faster than entries expire). No background sweeper — a
/// map whose own access pattern provides the sweep opportunity does not need
/// another task to schedule, supervise and shut down.
pub struct RunEventRegistry {
    pending: Mutex<HashMap<TurnId, (mpsc::UnboundedReceiver<StreamEvent>, Instant)>>,
    completed: Mutex<HashMap<TurnId, Instant>>,
    clock: Arc<dyn Clock>,
}

impl Default for RunEventRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Prune `map` of entries older than [`RUN_EVENT_TTL`], then evict oldest-first
/// until it has room for one more, and report how many the cap evicted.
///
/// Eviction rather than refusal is deliberate: `register` hands back the
/// producer's sending half and has no failure channel that would not turn a
/// memory bound into a rejected submission. An entry old enough to be the
/// oldest of [`RUN_EVENT_MAX_ENTRIES`] is one whose client has, by
/// construction, already lost its 404 race.
fn prune_and_make_room<V>(map: &mut HashMap<TurnId, V>, now: Instant, at: fn(&V) -> Instant) {
    map.retain(|_, v| now.duration_since(at(v)) < RUN_EVENT_TTL);
    while map.len() >= RUN_EVENT_MAX_ENTRIES {
        let Some(oldest) = map
            .iter()
            .min_by_key(|(_, v)| at(v))
            .map(|(k, _)| *k)
        else {
            break;
        };
        map.remove(&oldest);
        tracing::error!(
            turn_id = %oldest,
            cap = RUN_EVENT_MAX_ENTRIES,
            "api-server run-event registry is at its cap with nothing expired to evict — \
             dropping the oldest entry. Its client can no longer open the stream anyway; \
             this bounds memory rather than refusing a legitimate submission."
        );
    }
}

impl RunEventRegistry {
    pub fn new() -> Self {
        Self::with_clock(Arc::new(SystemClock))
    }

    /// Injectable-clock constructor, so the TTL can be exercised
    /// deterministically instead of by sleeping (source_facts #7 — this
    /// platform has no GNU `timeout` and fresh test binaries can stall in the
    /// dynamic loader). Mirrors [`crate::webhook::WebhookAdapter::new_with_clock`].
    pub fn with_clock(clock: Arc<dyn Clock>) -> Self {
        Self {
            pending: Mutex::new(HashMap::new()),
            completed: Mutex::new(HashMap::new()),
            clock,
        }
    }

    /// Create a fresh channel for `turn_id`, store the receiving half, and return
    /// the sending half to the caller (the run's producer task).
    pub async fn register(&self, turn_id: TurnId) -> mpsc::UnboundedSender<StreamEvent> {
        let (tx, rx) = mpsc::unbounded_channel();
        let now = self.clock.now();
        let mut pending = self.pending.lock().await;
        prune_and_make_room(&mut pending, now, |(_, at)| *at);
        pending.insert(turn_id, (rx, now));
        tx
    }

    /// Take ownership of the receiving half for `turn_id`, if one exists and has not
    /// already been taken.
    pub async fn take(&self, turn_id: TurnId) -> Option<mpsc::UnboundedReceiver<StreamEvent>> {
        self.pending.lock().await.remove(&turn_id).map(|(rx, _)| rx)
    }

    /// Mark `turn_id` as having completed (called by the producer after it
    /// deregisters from the turn registry) — lets the status route report
    /// "completed" instead of collapsing a finished run into "not found".
    ///
    /// WR-01: also drops any receiver still sitting in `pending` for this run.
    /// This is the reclamation the normal path needs, and it is safe because
    /// the `events` route requires a live turn-registry entry to open at all
    /// and the producer deregisters immediately after this call — the stream
    /// is unopenable from here on either way, so the buffered events have no
    /// remaining consumer. The TTL and cap above remain the backstop for the
    /// producer that never reaches this line (panic, abort, cancellation).
    pub async fn mark_completed(&self, turn_id: TurnId) {
        let now = self.clock.now();
        self.pending.lock().await.remove(&turn_id);
        let mut completed = self.completed.lock().await;
        prune_and_make_room(&mut completed, now, |at| *at);
        completed.insert(turn_id, now);
    }

    /// Whether `turn_id` was previously marked completed.
    pub async fn is_completed(&self, turn_id: TurnId) -> bool {
        let now = self.clock.now();
        let mut completed = self.completed.lock().await;
        completed.retain(|_, at| now.duration_since(*at) < RUN_EVENT_TTL);
        completed.contains_key(&turn_id)
    }

    /// Live entry counts — used by tests to assert both maps bound themselves
    /// without an external sweeper. Returns `(pending, completed)`.
    #[doc(hidden)]
    pub async fn entry_counts(&self) -> (usize, usize) {
        (
            self.pending.lock().await.len(),
            self.completed.lock().await.len(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::webhook::idempotency::FakeClock;

    // ── WR-01: both registry maps reclaim, and are bounded ───────────────────

    fn run_id(n: u128) -> TurnId {
        TurnId::from_u128(n)
    }

    /// The normal path. `run_stub_turn` finishes in ~150 ms, and the `events`
    /// route refuses with 404 unless the turn registry still holds the entry —
    /// so a client that submits and opens the stream a moment later gets a 404
    /// AND, before this fix, left the receiver in `pending` forever holding
    /// the whole echoed prompt. Nobody ever calls `take` on it again.
    #[tokio::test]
    async fn a_completed_run_reclaims_its_pending_receiver() {
        let reg = RunEventRegistry::new();
        let id = run_id(1);
        let _tx = reg.register(id).await;
        assert_eq!(reg.entry_counts().await.0, 1, "registered while in flight");

        reg.mark_completed(id).await;
        assert_eq!(
            reg.entry_counts().await.0,
            0,
            "a completed run must not leave its buffered event stream retained — its \
             client can no longer open the stream, so there is no remaining consumer"
        );
        assert!(
            reg.take(id).await.is_none(),
            "and the receiver must genuinely be gone, not merely uncounted"
        );
        assert!(
            reg.is_completed(id).await,
            "reclaiming the receiver must not lose the completion fact — the status \
             route still has to report 'completed' rather than collapsing to 404"
        );
    }

    /// The backstop for the producer that never reaches `mark_completed` at
    /// all — a panicking, aborted or cancelled task. Driven by an injected
    /// clock, never by sleeping (source_facts #7).
    #[tokio::test]
    async fn abandoned_pending_entries_expire_without_an_external_sweeper() {
        let clock = Arc::new(FakeClock::new());
        let reg = RunEventRegistry::with_clock(clock.clone() as Arc<dyn Clock>);

        for i in 0..50 {
            // The sender is dropped immediately — exactly the producer that
            // died without marking completion.
            let _ = reg.register(run_id(i)).await;
        }
        assert_eq!(
            reg.entry_counts().await.0,
            50,
            "entries are retained while live — a run still producing must be streamable"
        );

        clock.advance(Duration::from_secs(3601));
        let _tx = reg.register(run_id(999)).await;
        assert_eq!(
            reg.entry_counts().await.0,
            1,
            "past the TTL, the next register sweeps every stale entry — leaving only \
             the fresh one"
        );
    }

    /// `completed` was never drained AT ALL — an unbounded set growing with
    /// lifetime run count for the process lifetime.
    #[tokio::test]
    async fn completed_entries_expire_too() {
        let clock = Arc::new(FakeClock::new());
        let reg = RunEventRegistry::with_clock(clock.clone() as Arc<dyn Clock>);

        for i in 0..50 {
            let _ = reg.register(run_id(i)).await;
            reg.mark_completed(run_id(i)).await;
        }
        assert_eq!(reg.entry_counts().await.1, 50);
        assert!(reg.is_completed(run_id(0)).await);

        clock.advance(Duration::from_secs(3601));
        assert!(
            !reg.is_completed(run_id(0)).await,
            "past the TTL a completed run is forgotten — the map cannot grow forever"
        );
        assert_eq!(
            reg.entry_counts().await.1,
            0,
            "and the read itself is what sweeps, with no background task"
        );
    }

    /// The cap arm: sustained arrival faster than the TTL expires anything is
    /// exactly the case the TTL cannot cover, so it is the arm that runs under
    /// attack. No clock advance here, deliberately — every entry stays live,
    /// so `retain` evicts nothing and the cap is the only thing standing.
    #[tokio::test]
    async fn the_cap_bounds_pending_when_nothing_can_expire() {
        // The real clock, deliberately: a `FakeClock` never advances on its
        // own, so every entry would carry an identical instant and
        // "oldest-first" would be meaningless. The hour-long TTL expires
        // nothing during this test either way, which is the condition under
        // test — the cap is the only thing standing.
        let reg = RunEventRegistry::new();

        for i in 0..(RUN_EVENT_MAX_ENTRIES as u128 + 5) {
            let _ = reg.register(run_id(i)).await;
            assert!(
                reg.entry_counts().await.0 <= RUN_EVENT_MAX_ENTRIES,
                "the pending map must never exceed its cap, not even transiently"
            );
        }
        assert_eq!(
            reg.entry_counts().await.0,
            RUN_EVENT_MAX_ENTRIES,
            "at the cap, an arriving run evicts the oldest rather than growing the map"
        );
        // Eviction is oldest-first, so the most recent submissions — the ones
        // whose clients might still be opening a stream — are the survivors.
        assert!(
            reg.take(run_id(RUN_EVENT_MAX_ENTRIES as u128 + 4))
                .await
                .is_some(),
            "the newest entry must survive the cap"
        );
        assert!(
            reg.take(run_id(0)).await.is_none(),
            "the oldest entry is the one evicted"
        );
    }

    #[test]
    fn sse_frame_serializes_with_a_type_tag_drawn_from_the_fixed_set() {
        let cases = [
            (
                SseFrame::ContentDelta {
                    text: "hi".to_string(),
                },
                "content_delta",
            ),
            (
                SseFrame::ToolCallDelta {
                    index: 0,
                    id: None,
                    name: None,
                    arguments: None,
                },
                "tool_call_delta",
            ),
            (
                SseFrame::Done { reason: None },
                "done",
            ),
            (
                SseFrame::ProviderError {
                    message: "boom".to_string(),
                },
                "provider_error",
            ),
        ];
        for (frame, expected_type) in cases {
            let json: serde_json::Value =
                serde_json::from_str(&serde_json::to_string(&frame).unwrap()).unwrap();
            assert_eq!(json["type"], expected_type);
        }
    }

    #[test]
    fn stream_event_variants_map_one_to_one_onto_sse_frame_variants() {
        // Source assertion companion: one arm per StreamEvent variant, no
        // fallback/wildcard arm that could silently swallow a future addition.
        let events = [
            StreamEvent::ContentDelta("x".to_string()),
            StreamEvent::ToolCallDelta {
                index: 0,
                id: None,
                name: None,
                arguments: None,
            },
            StreamEvent::Usage(ironhermes_core::Usage {
                prompt_tokens: 1,
                completion_tokens: 1,
                total_tokens: 2,
                cache_read_input_tokens: None,
                cache_creation_input_tokens: None,
            }),
            StreamEvent::Done(None),
            StreamEvent::ProviderError("e".to_string()),
        ];
        for event in &events {
            let _frame: SseFrame = event.into();
        }
    }
}
