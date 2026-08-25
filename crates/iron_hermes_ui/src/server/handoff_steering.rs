//! Phase 50.2 Plan 16 (G-50.2-2c): the subprocess transport's own next-turn
//! steering primitive — the accept-and-queue analogue of the LIVE profile's
//! embedded-runtime mid-turn queue (`ws.rs:931-957`, R39.1-06).
//!
//! **Process-wide and session-transient.** State here lives in a single
//! module-level `Mutex`, never on disk. A process restart clears every
//! in-flight marker and every queued message — nothing in this module is
//! persisted, unlike a Bot Chat thread's own entries (which persist through
//! `chat_window.rs`'s `append_bot_chat_entry`, independent of this module).
//!
//! **Never spawns a turn of its own.** A queued message rides the bot's
//! NEXT handoff, whichever surface starts it — a Bot Chat send, an
//! `@mention` handoff, or the bot's next group-room round turn. Nothing in
//! this module calls `run_bot_handoff*`, and nothing here reaches into
//! `app_state.queue` — the LIVE profile's OWN separate mid-turn queue
//! (R39.1-06), which serves a different transport and is not touched from
//! here (this plan's own prohibition).
//!
//! **Synchronous only.** Every function in this module holds the module
//! mutex for a handful of map operations and never awaits while holding it
//! — no lock here can ever span an `.await`.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

/// Phase 50.2 Plan 16 (G-50.2-2c): the per-bot cap on how many steering
/// messages can wait for a bot's next turn. A steering queue is an operator
/// typing ahead while a turn runs — not a message bus — so a small bound is
/// enough to hold a burst of "wait, also do X" follow-ups while still
/// refusing (loudly, via [`SteeringError::QueueFull`]) a runaway client that
/// tries to grow it without limit.
#[cfg(feature = "server")]
pub(crate) const STEERING_QUEUE_MAX: usize = 8;

/// Phase 50.2 Plan 16 (G-50.2-2c): every failure mode [`begin_turn_or_queue`]
/// can return. Hand-written `Display`, matching
/// [`crate::server::cli_handoff::BotHandoffError`]'s style —
/// operator-readable, carrying no bot output.
#[cfg(feature = "server")]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SteeringError {
    QueueFull { max: usize },
    EmptyMessage,
}

#[cfg(feature = "server")]
impl std::fmt::Display for SteeringError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SteeringError::QueueFull { max } => write!(
                f,
                "this bot already has {max} steering messages waiting — wait for its next turn before sending another"
            ),
            SteeringError::EmptyMessage => {
                write!(f, "cannot send an empty message")
            }
        }
    }
}

#[cfg(feature = "server")]
impl std::error::Error for SteeringError {}

/// Phase 50.2 Plan 16 (G-50.2-2c): process-wide steering state.
///
/// `in_flight` is a REFERENCE COUNT, not a set — the same bot can
/// legitimately have two turns in flight at once (one per room, or a group
/// round turn racing a direct Bot Chat send), and a set would clear the
/// "busy" flag the moment the FIRST of those finishes, wrongly letting a
/// second concurrent send believe the bot is idle while the other turn is
/// still running.
///
/// Phase 50.2 Plan 18 (G-50.2-2c): `room_in_flight` and `room_queues` are the
/// ROOM-scoped twin of `in_flight`/`queues`, in their OWN maps keyed by room
/// id — a bot name and a room slug that happen to share a string can never
/// read or clobber each other's entries.
#[cfg(feature = "server")]
struct SteeringState {
    in_flight: HashMap<String, u32>,
    queues: HashMap<String, Vec<String>>,
    room_in_flight: HashMap<String, u32>,
    room_queues: HashMap<String, Vec<String>>,
}

#[cfg(feature = "server")]
fn state() -> &'static Mutex<SteeringState> {
    static STATE: OnceLock<Mutex<SteeringState>> = OnceLock::new();
    STATE.get_or_init(|| {
        Mutex::new(SteeringState {
            in_flight: HashMap::new(),
            queues: HashMap::new(),
            room_in_flight: HashMap::new(),
            room_queues: HashMap::new(),
        })
    })
}

/// Phase 50.2 Plan 16 (G-50.2-2c): RAII guard marking one bot as having a
/// turn in flight. `Drop` is the ONLY way the reference count goes down, so
/// an error, a timeout, a cancellation, or an early return all release it —
/// there is no path that can leak a permanently-busy bot short of the
/// process itself dying with the guard still held (in which case the whole
/// process-wide state resets on restart anyway).
#[cfg(feature = "server")]
#[derive(Debug)]
pub(crate) struct InFlightGuard {
    bot: String,
}

#[cfg(feature = "server")]
impl Drop for InFlightGuard {
    fn drop(&mut self) {
        let mut guard = state().lock().unwrap_or_else(|e| e.into_inner());
        if let Some(count) = guard.in_flight.get_mut(&self.bot) {
            if *count <= 1 {
                guard.in_flight.remove(&self.bot);
            } else {
                *count -= 1;
            }
        }
    }
}

/// Phase 50.2 Plan 16 (G-50.2-2c): unconditional in-flight marker for
/// callers that must run NOW — a group-round member turn or an `@mention`
/// handoff. These never queue (queueing a member's own round turn would
/// deadlock the drive), so they mark themselves busy directly rather than
/// going through [`begin_turn_or_queue`]'s run-or-queue decision.
#[cfg(feature = "server")]
pub(crate) fn mark_bot_in_flight(bot: &str) -> InFlightGuard {
    let mut guard = state().lock().unwrap_or_else(|e| e.into_inner());
    *guard.in_flight.entry(bot.to_string()).or_insert(0) += 1;
    InFlightGuard {
        bot: bot.to_string(),
    }
}

/// Phase 50.2 Plan 16 (G-50.2-2c): the outcome of [`begin_turn_or_queue`] —
/// either the caller may run the turn now (holding the returned guard for
/// its whole duration), or the message was accepted and queued for the
/// bot's next turn at the returned depth.
#[cfg(feature = "server")]
#[derive(Debug)]
pub(crate) enum BeginOrQueue {
    Begin(InFlightGuard),
    Queued { depth: u32 },
}

/// Phase 50.2 Plan 16 (G-50.2-2c): ONE lock hold that either (a) finds the
/// bot idle, increments its in-flight count, and returns the guard, or (b)
/// finds it busy, pushes the message onto its queue, and returns the new
/// depth. This atomicity is the entire point — two simultaneous sends can
/// never both observe an idle bot and both decide to run now, because the
/// check and the state mutation happen inside the SAME mutex hold, not a
/// check-then-act pair with a window between them.
///
/// An empty or whitespace-only message is refused before the lock is even
/// taken — it can never reach the queue and can never flip a bot from idle
/// to busy.
#[cfg(feature = "server")]
pub(crate) fn begin_turn_or_queue(
    bot: &str,
    message: &str,
) -> Result<BeginOrQueue, SteeringError> {
    if message.trim().is_empty() {
        return Err(SteeringError::EmptyMessage);
    }

    let mut guard = state().lock().unwrap_or_else(|e| e.into_inner());
    let busy = guard.in_flight.get(bot).copied().unwrap_or(0) > 0;
    if busy {
        let queue = guard.queues.entry(bot.to_string()).or_default();
        if queue.len() >= STEERING_QUEUE_MAX {
            return Err(SteeringError::QueueFull {
                max: STEERING_QUEUE_MAX,
            });
        }
        queue.push(message.to_string());
        let depth = queue.len() as u32;
        Ok(BeginOrQueue::Queued { depth })
    } else {
        *guard.in_flight.entry(bot.to_string()).or_insert(0) += 1;
        Ok(BeginOrQueue::Begin(InFlightGuard {
            bot: bot.to_string(),
        }))
    }
}

/// Phase 50.2 Plan 16 (G-50.2-2c): drain and return every steering message
/// queued for `bot`, in enqueue order, leaving its queue empty. Keyed by the
/// validated bot name — no cross-bot read path exists.
#[cfg(feature = "server")]
pub(crate) fn drain_steering(bot: &str) -> Vec<String> {
    let mut guard = state().lock().unwrap_or_else(|e| e.into_inner());
    guard.queues.remove(bot).unwrap_or_default()
}

/// Phase 50.2 Plan 16 (G-50.2-2c): how many steering messages are currently
/// waiting for `bot`'s next turn. No production call site reaches this fn —
/// `BotDispatchOutcome::Queued`'s own `depth` field (returned by
/// `begin_turn_or_queue`'s `Queued` branch) is what a production caller
/// reads instead — but it is genuinely exercised by this module's own tests
/// and by `cli_handoff.rs`'s end-to-end steering tests (asserting the queue
/// drains to zero), matching this crate's `#[cfg_attr(not(test), ...)]`
/// precedent (`cli_handoff.rs`'s own `run_bot_handoff`) for a fn that is
/// test-pinned rather than production-dead.
#[cfg_attr(not(test), allow(dead_code))]
#[cfg(feature = "server")]
pub(crate) fn steering_depth(bot: &str) -> u32 {
    let guard = state().lock().unwrap_or_else(|e| e.into_inner());
    guard.queues.get(bot).map(|q| q.len() as u32).unwrap_or(0)
}

/// Phase 50.2 Plan 16 (G-50.2-2c): PURE, no lock, no I/O. An empty `queued`
/// returns `message.to_string()` and nothing else — the byte-for-byte
/// identity that keeps every existing handoff and group-round test passing
/// with unmodified assertions when a bot has no waiting steering messages.
/// A non-empty `queued` renders each entry on its own attributed operator
/// line (in enqueue order), then a blank line, then the original message
/// verbatim — the attribution tells the model these lines arrived while it
/// was working and are steering for THIS turn, not the original request.
#[cfg(feature = "server")]
pub(crate) fn fold_steering_into_prompt(queued: &[String], message: &str) -> String {
    if queued.is_empty() {
        return message.to_string();
    }
    let mut out = String::new();
    for text in queued {
        out.push_str("[Operator sent this while you were working — steering for this turn]: ");
        out.push_str(text);
        out.push('\n');
    }
    out.push('\n');
    out.push_str(message);
    out
}

// ---------------------------------------------------------------------
// Phase 50.2 Plan 18 (G-50.2-2c): the ROOM-scoped twin of the bot-scoped
// surface above. Same `SteeringState`, own maps (`room_in_flight`,
// `room_queues`), same `STEERING_QUEUE_MAX` cap and `SteeringError` type —
// no second cap constant, no second error type, no second steering store.
// ---------------------------------------------------------------------

/// Phase 50.2 Plan 18 (G-50.2-2c): RAII guard marking one room as having a
/// round drive in flight. `Drop` is the ONLY way its reference count goes
/// down — the room-level twin of [`InFlightGuard`], for the same reason: an
/// error, an early return, or any other exit path from
/// `run_group_rounds_with_settings` must release the room exactly once, and
/// tying release to `Drop` rather than a trailing statement is what
/// guarantees that regardless of which exit path is taken.
#[cfg(feature = "server")]
#[derive(Debug)]
pub(crate) struct RoomDriveGuard {
    room: String,
}

#[cfg(feature = "server")]
impl Drop for RoomDriveGuard {
    fn drop(&mut self) {
        let mut guard = state().lock().unwrap_or_else(|e| e.into_inner());
        if let Some(count) = guard.room_in_flight.get_mut(&self.room) {
            if *count <= 1 {
                guard.room_in_flight.remove(&self.room);
            } else {
                *count -= 1;
            }
        }
    }
}

/// Phase 50.2 Plan 18 (G-50.2-2c): the outcome of
/// [`begin_room_drive_or_queue`] — either the caller may run the round
/// drive now (holding the returned guard for the whole drive), or the
/// operator's message was accepted and QUEUED for injection at the running
/// drive's next round boundary at the returned depth. The room-level twin
/// of [`BeginOrQueue`].
#[cfg(feature = "server")]
#[derive(Debug)]
pub(crate) enum BeginRoomDriveOrQueue {
    Begin(RoomDriveGuard),
    Queued { depth: u32 },
}

/// Phase 50.2 Plan 18 (G-50.2-2c): pushes `message` onto `room_id`'s queue,
/// enforcing [`STEERING_QUEUE_MAX`] — the one place the room queue's cap
/// check lives, called only while [`state`]'s lock is already held (never
/// re-acquires it, so it can be called from inside
/// [`begin_room_drive_or_queue`]'s own lock hold as well as from
/// [`enqueue_room_steering`]).
#[cfg(feature = "server")]
fn push_room_queue(
    locked: &mut SteeringState,
    room_id: &str,
    message: &str,
) -> Result<u32, SteeringError> {
    let queue = locked.room_queues.entry(room_id.to_string()).or_default();
    if queue.len() >= STEERING_QUEUE_MAX {
        return Err(SteeringError::QueueFull {
            max: STEERING_QUEUE_MAX,
        });
    }
    queue.push(message.to_string());
    Ok(queue.len() as u32)
}

/// Phase 50.2 Plan 18 (G-50.2-2c): ONE lock hold that either (a) finds the
/// room's drive idle, increments its in-flight count, and returns the
/// guard, or (b) finds a drive already running for it, pushes the operator's
/// message onto its queue, and returns the new depth. Same atomicity
/// contract as [`begin_turn_or_queue`]: the busy check and the state
/// mutation happen inside the SAME mutex hold, so two simultaneous sends
/// for the same room can never both observe an idle room and both decide to
/// start a drive — this is what makes T-50.2-18-01 (two concurrent drives
/// on one room) structurally impossible rather than merely unlikely.
///
/// An empty or whitespace-only message is refused before the lock is even
/// taken — it can never reach the queue and can never flip a room from idle
/// to busy.
#[cfg(feature = "server")]
pub(crate) fn begin_room_drive_or_queue(
    room_id: &str,
    message: &str,
) -> Result<BeginRoomDriveOrQueue, SteeringError> {
    if message.trim().is_empty() {
        return Err(SteeringError::EmptyMessage);
    }

    let mut guard = state().lock().unwrap_or_else(|e| e.into_inner());
    let busy = guard.room_in_flight.get(room_id).copied().unwrap_or(0) > 0;
    if busy {
        let depth = push_room_queue(&mut guard, room_id, message)?;
        Ok(BeginRoomDriveOrQueue::Queued { depth })
    } else {
        *guard.room_in_flight.entry(room_id.to_string()).or_insert(0) += 1;
        Ok(BeginRoomDriveOrQueue::Begin(RoomDriveGuard {
            room: room_id.to_string(),
        }))
    }
}

/// Phase 50.2 Plan 18 (G-50.2-2c): unconditionally enqueue `message` for
/// `room_id`, regardless of whether a drive is currently marked in flight
/// for that room. Does NOT consult or mutate `room_in_flight` — this is
/// deliberate: the round-driver's own tests seed a room's queue directly
/// (bypassing [`begin_room_drive_or_queue`]'s locking dance entirely) to
/// exercise `run_group_rounds_with_settings`'s round-boundary drain in
/// isolation from the `#[server]` dispatch layer. Same cap, same error type
/// as [`begin_room_drive_or_queue`]'s busy branch — both funnel through
/// [`push_room_queue`]. No production call site exists on the native "bin"
/// target — `begin_room_drive_or_queue`'s own busy branch calls
/// [`push_room_queue`] directly rather than this fn (re-acquiring the lock
/// from inside an already-held lock would deadlock a `std::sync::Mutex`) —
/// but it is genuinely exercised by this module's own tests and by
/// `group_chat_api.rs`'s round-driver tests, matching this crate's
/// `#[cfg_attr(not(test), ...)]` precedent (`cli_handoff.rs`'s own
/// `run_bot_handoff`, this module's own `steering_depth`) for a fn that is
/// test-pinned rather than production-dead.
#[cfg_attr(not(test), allow(dead_code))]
#[cfg(feature = "server")]
pub(crate) fn enqueue_room_steering(room_id: &str, message: &str) -> Result<u32, SteeringError> {
    if message.trim().is_empty() {
        return Err(SteeringError::EmptyMessage);
    }

    let mut guard = state().lock().unwrap_or_else(|e| e.into_inner());
    push_room_queue(&mut guard, room_id, message)
}

/// Phase 50.2 Plan 18 (G-50.2-2c): drain and return every steering message
/// queued for `room_id`, in enqueue order, leaving its queue empty. Called
/// once at the top of every round in `run_group_rounds_with_settings` — the
/// room-level twin of [`drain_steering`].
#[cfg(feature = "server")]
pub(crate) fn drain_room_steering(room_id: &str) -> Vec<String> {
    let mut guard = state().lock().unwrap_or_else(|e| e.into_inner());
    guard.room_queues.remove(room_id).unwrap_or_default()
}

/// Phase 50.2 Plan 18 (G-50.2-2c): how many steering messages are currently
/// waiting for `room_id`'s next round boundary. Exercised by this module's
/// own tests and by `group_chat_api.rs`'s round-driver tests (asserting the
/// queue drains to zero after injection) — the room-level twin of
/// [`steering_depth`], same test-pinned-not-production-dead precedent.
#[cfg_attr(not(test), allow(dead_code))]
#[cfg(feature = "server")]
pub(crate) fn room_steering_depth(room_id: &str) -> u32 {
    let guard = state().lock().unwrap_or_else(|e| e.into_inner());
    guard
        .room_queues
        .get(room_id)
        .map(|q| q.len() as u32)
        .unwrap_or(0)
}

/// Phase 50.2 Plan 16 (G-50.2-2c): clears every map, bot-scoped and
/// room-scoped alike. The state is process-wide, and this crate's tests
/// already serialise on [`crate::server::test_support::env_lock`] — every
/// test that touches this module's state must hold that lock AND call this
/// fn in its preamble so it starts from a known-empty state regardless of
/// test execution order.
#[cfg(all(test, feature = "server"))]
pub(crate) fn reset_steering_for_test() {
    let mut guard = state().lock().unwrap_or_else(|e| e.into_inner());
    guard.in_flight.clear();
    guard.queues.clear();
    guard.room_in_flight.clear();
    guard.room_queues.clear();
}

#[cfg(all(test, feature = "server"))]
mod tests {
    use super::*;

    // -------------------------------------------------------------------
    // begin_turn_or_queue
    // -------------------------------------------------------------------

    #[test]
    fn begin_turn_or_queue_returns_begin_when_bot_is_idle() {
        let _lock = crate::server::test_support::env_lock();
        reset_steering_for_test();

        match begin_turn_or_queue("scout-steering-idle", "hello").expect("must not error") {
            BeginOrQueue::Begin(_guard) => {}
            BeginOrQueue::Queued { .. } => panic!("an idle bot must return Begin, not Queued"),
        }
    }

    #[test]
    fn begin_turn_or_queue_returns_queued_with_incrementing_depth_when_busy() {
        let _lock = crate::server::test_support::env_lock();
        reset_steering_for_test();

        let bot = "scout-steering-busy";
        let _guard = mark_bot_in_flight(bot);

        match begin_turn_or_queue(bot, "first steer").expect("must not error") {
            BeginOrQueue::Queued { depth } => assert_eq!(depth, 1),
            BeginOrQueue::Begin(_) => panic!("a busy bot must queue, not begin"),
        }
        match begin_turn_or_queue(bot, "second steer").expect("must not error") {
            BeginOrQueue::Queued { depth } => assert_eq!(depth, 2),
            BeginOrQueue::Begin(_) => panic!("a busy bot must queue, not begin"),
        }
    }

    #[test]
    fn in_flight_reference_count_keeps_bot_busy_until_every_guard_drops() {
        let _lock = crate::server::test_support::env_lock();
        reset_steering_for_test();

        let bot = "scout-steering-refcount";
        let first = mark_bot_in_flight(bot);
        let _second = mark_bot_in_flight(bot);

        drop(first);
        // One guard remains — the bot must still read as busy.
        match begin_turn_or_queue(bot, "should still queue").expect("must not error") {
            BeginOrQueue::Queued { depth } => assert_eq!(depth, 1),
            BeginOrQueue::Begin(_) => {
                panic!("bot must still be busy while a second guard is held")
            }
        }
    }

    #[test]
    fn dropping_the_last_in_flight_guard_makes_the_bot_idle_again() {
        let _lock = crate::server::test_support::env_lock();
        reset_steering_for_test();

        let bot = "scout-steering-idle-after-drop";
        let guard = mark_bot_in_flight(bot);
        drop(guard);

        match begin_turn_or_queue(bot, "hello again").expect("must not error") {
            BeginOrQueue::Begin(_guard) => {}
            BeginOrQueue::Queued { .. } => {
                panic!("bot must be idle again once its only guard is dropped")
            }
        }
    }

    #[test]
    fn dropping_the_guard_makes_the_bot_idle_even_after_an_error_or_cancellation() {
        // The guard's Drop is the ONLY release mechanism, so it must not
        // matter WHY the guard is being dropped — an error path or a
        // cancellation drops it exactly the same way a success path does.
        let _lock = crate::server::test_support::env_lock();
        reset_steering_for_test();

        let bot = "scout-steering-idle-after-error";
        {
            let _guard = mark_bot_in_flight(bot);
            // Simulate an error path: the guard just goes out of scope here.
        }

        match begin_turn_or_queue(bot, "hello").expect("must not error") {
            BeginOrQueue::Begin(_guard) => {}
            BeginOrQueue::Queued { .. } => {
                panic!("bot must be idle again once its guard is dropped via any exit path")
            }
        }
    }

    // -------------------------------------------------------------------
    // enqueue cap + empty-message refusal
    // -------------------------------------------------------------------

    #[test]
    fn enqueueing_past_the_cap_is_refused_and_does_not_grow_the_queue() {
        let _lock = crate::server::test_support::env_lock();
        reset_steering_for_test();

        let bot = "scout-steering-cap";
        let _guard = mark_bot_in_flight(bot);

        for i in 0..STEERING_QUEUE_MAX {
            begin_turn_or_queue(bot, &format!("msg {i}")).expect("must accept up to the cap");
        }
        assert_eq!(steering_depth(bot), STEERING_QUEUE_MAX as u32);

        let err = begin_turn_or_queue(bot, "one too many").expect_err("must refuse past the cap");
        assert_eq!(
            err,
            SteeringError::QueueFull {
                max: STEERING_QUEUE_MAX
            }
        );
        assert_eq!(
            steering_depth(bot),
            STEERING_QUEUE_MAX as u32,
            "a refused enqueue must not grow the queue"
        );
    }

    #[test]
    fn empty_or_whitespace_only_message_is_refused_before_it_reaches_the_queue() {
        let _lock = crate::server::test_support::env_lock();
        reset_steering_for_test();

        let bot = "scout-steering-empty";
        let _guard = mark_bot_in_flight(bot);

        assert_eq!(
            begin_turn_or_queue(bot, "").unwrap_err(),
            SteeringError::EmptyMessage
        );
        assert_eq!(
            begin_turn_or_queue(bot, "   ").unwrap_err(),
            SteeringError::EmptyMessage
        );
        assert_eq!(
            steering_depth(bot),
            0,
            "a refused empty message must never reach the queue"
        );
    }

    // -------------------------------------------------------------------
    // drain_steering / steering_depth
    // -------------------------------------------------------------------

    #[test]
    fn drain_steering_returns_queued_messages_in_enqueue_order_and_empties_the_queue() {
        let _lock = crate::server::test_support::env_lock();
        reset_steering_for_test();

        let bot = "scout-steering-drain";
        let _guard = mark_bot_in_flight(bot);
        begin_turn_or_queue(bot, "first").expect("queue first");
        begin_turn_or_queue(bot, "second").expect("queue second");

        let drained = drain_steering(bot);
        assert_eq!(drained, vec!["first".to_string(), "second".to_string()]);
        assert_eq!(steering_depth(bot), 0, "drain must empty the queue");
    }

    // -------------------------------------------------------------------
    // fold_steering_into_prompt (pure)
    // -------------------------------------------------------------------

    #[test]
    fn fold_steering_into_prompt_with_empty_queue_is_byte_for_byte_identity() {
        let message = "the original message, unmodified";
        assert_eq!(fold_steering_into_prompt(&[], message), message);
    }

    #[test]
    fn fold_steering_into_prompt_renders_each_queued_entry_above_the_message_in_order() {
        let queued = vec!["also check the logs".to_string(), "and ping ops".to_string()];
        let folded = fold_steering_into_prompt(&queued, "original message");

        let first_pos = folded.find("also check the logs").expect("first steer present");
        let second_pos = folded.find("and ping ops").expect("second steer present");
        let message_pos = folded
            .find("original message")
            .expect("original message present verbatim");

        assert!(
            first_pos < second_pos,
            "queued entries must render in enqueue order"
        );
        assert!(
            second_pos < message_pos,
            "every queued entry must render above the original message"
        );
        assert!(folded.ends_with("original message"));
    }

    // -------------------------------------------------------------------
    // Phase 50.2 Plan 18 (G-50.2-2c): begin_room_drive_or_queue
    // -------------------------------------------------------------------

    #[test]
    fn begin_room_drive_or_queue_returns_begin_when_room_is_idle() {
        let _lock = crate::server::test_support::env_lock();
        reset_steering_for_test();

        match begin_room_drive_or_queue("room-steering-idle", "hello room")
            .expect("must not error")
        {
            BeginRoomDriveOrQueue::Begin(_guard) => {}
            BeginRoomDriveOrQueue::Queued { .. } => {
                panic!("an idle room must return Begin, not Queued")
            }
        }
    }

    #[test]
    fn begin_room_drive_or_queue_returns_queued_with_incrementing_depth_when_busy() {
        let _lock = crate::server::test_support::env_lock();
        reset_steering_for_test();

        let room = "room-steering-busy";
        let guard = match begin_room_drive_or_queue(room, "kickoff").expect("must not error") {
            BeginRoomDriveOrQueue::Begin(guard) => guard,
            BeginRoomDriveOrQueue::Queued { .. } => panic!("first call on an idle room must Begin"),
        };

        match begin_room_drive_or_queue(room, "first steer").expect("must not error") {
            BeginRoomDriveOrQueue::Queued { depth } => assert_eq!(depth, 1),
            BeginRoomDriveOrQueue::Begin(_) => panic!("a busy room must queue, not begin"),
        }
        match begin_room_drive_or_queue(room, "second steer").expect("must not error") {
            BeginRoomDriveOrQueue::Queued { depth } => assert_eq!(depth, 2),
            BeginRoomDriveOrQueue::Begin(_) => panic!("a busy room must queue, not begin"),
        }
        drop(guard);
    }

    #[test]
    fn dropping_the_room_drive_guard_makes_the_room_drivable_again() {
        let _lock = crate::server::test_support::env_lock();
        reset_steering_for_test();

        let room = "room-steering-drivable-after-drop";
        let guard = match begin_room_drive_or_queue(room, "kickoff").expect("must not error") {
            BeginRoomDriveOrQueue::Begin(guard) => guard,
            BeginRoomDriveOrQueue::Queued { .. } => panic!("first call on an idle room must Begin"),
        };
        drop(guard);

        match begin_room_drive_or_queue(room, "second drive").expect("must not error") {
            BeginRoomDriveOrQueue::Begin(_guard) => {}
            BeginRoomDriveOrQueue::Queued { .. } => {
                panic!("room must be drivable again once its only guard is dropped")
            }
        }
    }

    #[test]
    fn dropping_the_room_drive_guard_makes_the_room_drivable_again_after_an_error() {
        // The guard's Drop is the ONLY release mechanism, so it must not
        // matter WHY the guard is being dropped — an error path releases it
        // exactly the same way a success path does.
        let _lock = crate::server::test_support::env_lock();
        reset_steering_for_test();

        let room = "room-steering-drivable-after-error";
        {
            let _guard = match begin_room_drive_or_queue(room, "kickoff").expect("must not error")
            {
                BeginRoomDriveOrQueue::Begin(guard) => guard,
                BeginRoomDriveOrQueue::Queued { .. } => {
                    panic!("first call on an idle room must Begin")
                }
            };
            // Simulate an error path: the guard just goes out of scope here.
        }

        match begin_room_drive_or_queue(room, "hello again").expect("must not error") {
            BeginRoomDriveOrQueue::Begin(_guard) => {}
            BeginRoomDriveOrQueue::Queued { .. } => {
                panic!("room must be drivable again once its guard is dropped via any exit path")
            }
        }
    }

    // -------------------------------------------------------------------
    // Phase 50.2 Plan 18 (G-50.2-2c): room queue cap reuse + key
    // independence from the bot-scoped map
    // -------------------------------------------------------------------

    #[test]
    fn enqueueing_past_the_cap_is_refused_for_a_room_and_does_not_grow_the_queue() {
        let _lock = crate::server::test_support::env_lock();
        reset_steering_for_test();

        let room = "room-steering-cap";
        let guard = match begin_room_drive_or_queue(room, "kickoff").expect("must not error") {
            BeginRoomDriveOrQueue::Begin(guard) => guard,
            BeginRoomDriveOrQueue::Queued { .. } => panic!("first call on an idle room must Begin"),
        };

        for i in 0..STEERING_QUEUE_MAX {
            enqueue_room_steering(room, &format!("msg {i}")).expect("must accept up to the cap");
        }
        assert_eq!(room_steering_depth(room), STEERING_QUEUE_MAX as u32);

        let err = enqueue_room_steering(room, "one too many").expect_err("must refuse past the cap");
        assert_eq!(
            err,
            SteeringError::QueueFull {
                max: STEERING_QUEUE_MAX
            }
        );
        assert_eq!(
            room_steering_depth(room),
            STEERING_QUEUE_MAX as u32,
            "a refused enqueue must not grow the queue"
        );
        drop(guard);
    }

    #[test]
    fn enqueue_room_steering_refuses_empty_or_whitespace_only_message() {
        let _lock = crate::server::test_support::env_lock();
        reset_steering_for_test();

        let room = "room-steering-empty";
        assert_eq!(
            enqueue_room_steering(room, "").unwrap_err(),
            SteeringError::EmptyMessage
        );
        assert_eq!(
            enqueue_room_steering(room, "   ").unwrap_err(),
            SteeringError::EmptyMessage
        );
        assert_eq!(
            room_steering_depth(room),
            0,
            "a refused empty message must never reach the queue"
        );
    }

    #[test]
    fn drain_room_steering_returns_queued_messages_in_enqueue_order_and_empties_the_queue() {
        let _lock = crate::server::test_support::env_lock();
        reset_steering_for_test();

        let room = "room-steering-drain";
        enqueue_room_steering(room, "first").expect("queue first");
        enqueue_room_steering(room, "second").expect("queue second");

        let drained = drain_room_steering(room);
        assert_eq!(drained, vec!["first".to_string(), "second".to_string()]);
        assert_eq!(room_steering_depth(room), 0, "drain must empty the queue");
    }

    #[test]
    fn room_queue_is_independent_of_the_bot_queue_for_a_shared_name() {
        // A bot named "shared-name" and a room slug "shared-name" must never
        // read or clobber each other's steering state — the room queue lives
        // in its OWN maps, keyed separately from the bot maps.
        let _lock = crate::server::test_support::env_lock();
        reset_steering_for_test();

        let shared = "shared-name";
        let _bot_guard = mark_bot_in_flight(shared);
        begin_turn_or_queue(shared, "bot-scoped steer").expect("queue for the bot");

        // The room of the same name must still read as idle — no bleed from
        // the bot's in-flight marker into `room_in_flight`.
        let room_guard = match begin_room_drive_or_queue(shared, "room-scoped kickoff")
            .expect("must not error")
        {
            BeginRoomDriveOrQueue::Begin(guard) => guard,
            BeginRoomDriveOrQueue::Queued { .. } => {
                panic!("the room's in-flight state must be independent of the bot's")
            }
        };

        // The room's own queue must be empty — the bot-scoped steer above
        // must not have landed in `room_queues`.
        assert_eq!(
            room_steering_depth(shared),
            0,
            "a bot-scoped steering message must never appear in the room queue"
        );
        // And draining the bot's queue must return only the bot-scoped
        // message — no bleed from `room_queues` into `queues`.
        assert_eq!(drain_steering(shared), vec!["bot-scoped steer".to_string()]);

        drop(room_guard);
    }
}
