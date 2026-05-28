# Phase 36.17.2.1 Research — Fix: /queue Slash-Command Fails to Wake Parked Worker

**Researched:** 2026-05-28
**Domain:** Tokio async concurrency — `tokio::sync::Notify` wake protocol, per-chat worker lifecycle
**Confidence:** HIGH (all claims derived from live source inspection)

---

## Reproduction & Evidence

### UAT Failure (2026-05-28T15:36–15:38 UTC)

128 of 129 `/queue <text>` commands produced the `"Queued for the next turn."` acknowledgment
(or the depth-aware `"Queued for the next turn. (N queued)"` form) but **never became LLM
turns**. Only the final plain `hello` was processed. Evidence of silent abandonment:

- SessionQueue depth rose from 0 to 128 over the test window — confirmed by the depth suffix
  appearing in bot replies as the commands accumulated.
- No 👀 reactions were emitted on any of the 128 queued messages. Under 36.17.2's architecture,
  👀 fires in the worker at pop-time (runner.rs:1066). Zero reactions means zero pops.
- The single `hello` message was processed — it arrived as a free-text event through
  `UQM::dispatch`, which calls `notify_one()` at `user_queue.rs:154`. That wake was the
  only `notify_one()` the worker ever received across the entire 129-message session.

### Smoking-Gun Grep

Every `notify_one()` call in the `crates/ironhermes-gateway/src/` tree:

```
user_queue.rs:154    notify.notify_one();
```

**Count: 1.** There is no `notify_one()` anywhere in `handler.rs`, `runner.rs`,
`session_queue.rs`, or any other gateway source file. The `/queue` handler at
`handler.rs:799–861` reaches `session_queue.try_push()` successfully but never reaches
`notify_one()`.

---

## Root Cause Map

### The Notify ownership graph (post-36.17.2)

```
UQM::workers  HashMap<SessionKey, Arc<Notify>>
     │
     ├── insert(key, notify)  ← at dispatch when no worker exists (user_queue.rs:159-160)
     ├── get(key).cloned()    ← returns Arc clone for signaling (user_queue.rs:151)
     │        └── notify.notify_one()  ← THE ONLY CALL SITE (user_queue.rs:154)
     └── remove(key)          ← on worker exit (user_queue.rs:179+)

runner.rs worker loop:
     notify_task = uqm.notify_for(&key).await   (runner.rs:1028-1031)
     loop {
         match session_queue.pop(&key) {
             None => select! {
                 _ = cancel.cancelled() => break,
                 _ = notify_task.notified() => continue,  ← PARKS HERE (runner.rs:1045)
             }
             Some(ev) => { /* process */ }
         }
     }
```

### Where Notify IS called

- **`user_queue.rs:154`** — `UQM::dispatch` → after successful `try_push` → acquires
  `workers` lock → finds existing `Arc<Notify>` → drops lock → calls `notify_one()`.
  This is the ONLY caller.

### Where Notify is NOT called (the bug)

- **`handler.rs:804-821`** — `CoreCommandResult::Queued` arm inside
  `handle_with_multimodal`. The code calls `queue.try_push(&session_key, queued_event)` at
  `handler.rs:805` via `self.session_queue.as_ref()`. `GatewayMessageHandler` holds
  `session_queue: Option<Arc<SessionQueue>>` (handler.rs:154) — a **bare `Arc<SessionQueue>`
  with no UQM reference**. There is no `Arc<UserQueueManager>` field anywhere in
  `GatewayMessageHandler`. The handler cannot call `notify_one()` without a code change.

### The dispatch path that causes the bug

```
runner.rs:935  if event.content.starts_with('/')  ← slash-command fast-path (D-23)
                 tokio::spawn(async move {
                   handler.handle_with_multimodal(event, ...)  ← enters handler
                 })
                 continue;  ← SKIPS UQM::dispatch entirely (runner.rs:970)

handler.rs:799  CoreCommandResult::Queued { message } =>
handler.rs:804    if let Some(queue) = self.session_queue.as_ref() {
handler.rs:805      queue.try_push(&session_key, queued_event)  ← push succeeds
                    // ... depth reply sent to user ...
                    // notify_one() NEVER CALLED
```

The worker is parked at `notify_task.notified()` (runner.rs:1045). No one wakes it.
The pushed event sits in SessionQueue indefinitely.

### Worker-spawn gap (secondary consequence)

The fast-path also skips the `DispatchOutcome::WorkerSpawned` arm
(runner.rs:1013-1121). If `/queue` arrives for a chat that has no active worker
(fresh chat, or post-cancel), `try_push` creates a queue entry but no worker exists
to ever pop it. Option B (route through `UQM::dispatch`) handles this automatically.
Option A requires the handler or runner to also spawn a worker when `notify_for` returns
`None`.

---

## Fix Space

### Option A — Surgical: add `notify_one()` (and conditional worker-spawn) in the handler

**What changes:**
1. `GatewayMessageHandler` gains an `Option<Arc<UserQueueManager>>` field alongside the
   existing `Option<Arc<SessionQueue>>`.
2. `build_gateway_handler` / `set_session_queue` is complemented by a
   `set_user_queue_manager` setter (or combined into one setter).
3. In the `CoreCommandResult::Queued` arm (handler.rs:804-821), after a successful
   `try_push`, call `uqm.notify_for(&session_key).await`:
   - If `Some(notify)` → `notify.notify_one()` (existing worker, needs wake).
   - If `None` → the worker has exited or never existed; call
     `uqm.dispatch(queued_event_clone, None, None).await` OR explicitly insert a new
     `Notify` and return `WorkerSpawned`, leaving the caller (runner.rs fast-path
     `tokio::spawn`) to spawn the worker. The second sub-option is complex from
     inside a detached spawn that has no access to `worker_join_set`.

**Files changed:** `handler.rs`, `user_queue.rs` (possibly), test setup helpers in
`uqm_session_queue_unification.rs` (need to pass UQM to handler in test harness).

**Invariants that must hold:**
- The `notify_one()` call must happen AFTER the `workers` mutex is dropped (D-19).
  `notify_for` already does `workers.lock().await; workers.get(key).cloned()` and
  returns without holding the lock — safe.
- The `std::sync::MutexGuard` from `try_push` must have dropped before any `.await`
  (D-18 / Pitfall 2). At handler.rs:805, `try_push` returns `Result<(), QueueError>`;
  the guard drops at the end of the `try_push` call. `notify_for` is then called as a
  separate `.await` — no overlap. Compiler enforces via `!Send`.
- The no-worker path must not silently drop the pushed event. If the handler calls
  `dispatch` as a fallback, `dispatch` will call `try_push` again — but the event
  was already pushed. Double-push corrupts FIFO order unless the handler clears the
  first push before calling `dispatch`. This is the primary ordering hazard for Option A.

**What could break:**
- Calling `dispatch` as fallback after `try_push` already succeeded = double-push.
  The only safe no-worker path is: (a) don't push via `try_push` at all — delegate
  entirely to `dispatch`; (b) push via `try_push`, then signal a new-worker-needed
  outcome back to runner.rs so it can spawn. Option (b) requires the detached fast-path
  `tokio::spawn` to have access to `worker_join_set`, which it currently does not.

**Verdict:** Viable for the common case (existing worker), but the no-worker path is
awkward. Requires adding UQM as a second optional field to `GatewayMessageHandler`,
complicating the handler's construction API and test harness.

---

### Option B — Architectural: replace `try_push` with `UQM::dispatch` in the handler

**What changes:**
1. `GatewayMessageHandler` gains `Option<Arc<UserQueueManager>>` (same as Option A,
   step 1-2).
2. The `CoreCommandResult::Queued` arm replaces the entire `session_queue.try_push`
   block with a call to `uqm.dispatch(queued_event, None, None).await`.
3. Map `DispatchOutcome` back to the existing depth-aware reply:
   - `Ok(Accepted | WorkerSpawned)` → call `queue.len(&session_key)` for the depth
     suffix, send reply. (Or: pre-compute depth from the queue after dispatch.)
   - `Err(CapacityReached)` → the ❌ reaction + "Queue is full" reply have already
     been sent by UQM::dispatch (D-11). The handler can return early silently, or
     send an additional "message dropped" reply — decision for planner (see Open Questions).
4. The `WorkerSpawned` outcome means UQM has inserted the Notify and expects the caller
   to spawn a worker. The handler is inside a detached fast-path spawn; it has no access
   to `worker_join_set_dispatch`. **Resolution:** The runner's fast-path spawn body must
   check the `DispatchOutcome` returned by `dispatch` and spawn a worker task. But the
   handler is called from inside that spawn — the outcome would need to propagate back
   up through `handle_with_multimodal`'s return value, or the runner must intercept
   before calling the handler.

   **Simpler alternative:** Move the `UQM::dispatch` call out of the handler entirely
   and into runner.rs's fast-path spawn, before calling `handle_with_multimodal`. The
   handler's `Queued` arm becomes unreachable from the fast-path. The handler retains
   the `try_push` path only for the legacy busy-branch (direct handler calls, existing
   tests). This is a cleaner separation: routing logic stays in runner.rs, handler stays
   stateless w.r.t. the worker registry.

**Files changed:** `handler.rs` (minimal — possible no-op if the interception is in
runner.rs), `runner.rs` (fast-path branch modified to intercept the `/queue` command
before it reaches `handle_with_multimodal`), `user_queue.rs` (no changes needed).

**Invariants that must hold:**
- `UQM::dispatch` serializes push + notify + worker-presence check under its own
  `tokio::sync::Mutex`. This is the correct lock to hold for the combined operation.
  No additional ordering concern.
- The `Queued` arm's `session_queue.try_push` path in handler.rs becomes dead code for
  the fast-path flow. It remains live for the busy-branch (direct handler calls). D-20
  says keep the busy-branch code — it is still exercised by `session_queue_integration.rs`
  tests. No removal needed.
- The cap-hit reply in the handler's `Queued` arm (handler.rs:823-843) becomes
  unreachable from the fast-path if UQM::dispatch intercepts first. The D-11 cap-hit
  UX (❌ + "Queue is full") fires from inside `UQM::dispatch` instead — same message,
  same emoji, different code path. The two cap-hit paths diverge only in test coverage.

**What could break:**
- The `depth` used for the depth-aware reply (`"Queued for the next turn. (N queued)"`)
  is computed with `queue.len(&session_key)` after the push (handler.rs:807). After
  option B's interception, depth is available via `session_queue.len()` or a new
  `UQM::queue_len()` wrapper. The reply wording is unchanged — just the call site moves.
- `test_slash_command_bypasses_per_chat_worker` (uqm_session_queue_unification.rs:835)
  asserts that the `/queue` intercept pushes a synthesized event and sends the "Queued
  for the next turn..." reply. If the interception moves to runner.rs, this test's
  assertion (b) — "SessionQueue grew from 1 to 2" — still holds. Assertion (c) — chat
  reply was sent — must still fire from wherever the reply is emitted.

**Verdict:** Architecturally cleaner. UQM owns the complete push+notify+spawn-signal
lifecycle for every event, regardless of entry point. The handler's `Queued` arm becomes
a thin reply-emitter, not a routing decision-maker.

---

### Option C — Runner intercepts `/queue` commands before handler dispatch

**What changes:**

Runner.rs's fast-path branch (runner.rs:935-970) currently routes ALL slash commands to
`handle_with_multimodal`. Option C adds a second check: if the slash command is
specifically `/queue <args>`, the runner extracts the args, synthesizes a `MessageEvent`
with `content = args`, and calls `uqm.dispatch(synthesized_event, None, None).await`
directly — without ever calling `handle_with_multimodal`. The `/queue` acknowledgment
("Queued for the next turn.") is then sent from runner.rs rather than from the handler's
`Queued` arm.

**Files changed:** `runner.rs` only (handler.rs untouched, user_queue.rs untouched).

**What this avoids:** Adding any UQM field to `GatewayMessageHandler`. The handler
remains unaware of the worker registry.

**What this breaks / complicates:**
- The command-parsing logic for `/queue` currently lives in the handler's
  `handle_slash_command` path (command registry at handler.rs:411+), which handles
  arg splitting, disambiguation, and the `CoreCommandResult::Queued` shape. Option C
  duplicates or bypasses this parsing. Any future changes to `/queue` arg parsing would
  need to update two places.
- Other slash commands that synthesize events (e.g., a future `/remind`) would not
  benefit from the fix — they would need the same runner-level interception pattern.
  Option B's `UQM::dispatch` call in the handler generalizes to all such commands.
- The runner already has access to `user_queue_dispatch` (an `Arc<UserQueueManager>`).
  Option C is therefore feasible without struct changes, but it hardcodes command-name
  knowledge into the runner, which is an architectural smell.

**Verdict:** Viable as a minimal-change option (handler.rs zero diff). However, it
splits command-parsing responsibility across two layers and does not generalize. Should
be considered only if Option B's handler change is blocked by test-surface concerns.

---

## Pitfalls

### 1. Race between `notify_one()` and worker parking

`tokio::sync::Notify` stores a permit when `notify_one()` is called and no waiter is
present (documented in tokio). If the `/queue` handler calls `notify_one()` before the
worker has reached `notify_task.notified()`, the permit is stored and the worker
consumes it on the next call — no lost wake. This is T-36.17.2-02 from CONTEXT.md and
is **not a hazard** for any of the three options.

However: if the worker is mid-turn (processing `handle_with_multimodal`) and a `/queue`
command arrives, `notify_one()` is called while the worker is not parked. The Notify
permit is banked. When the worker finishes the turn and loops back to the `None` branch
(queue empty check), it sees the banked permit from `notified()`, re-polls the queue,
and finds the `/queue` event. This is **correct behavior** and requires no mitigation.

### 2. No-worker path: `/queue` on a fresh chat

If the first message a user ever sends is `/queue something`, no worker exists for that
SessionKey. The fast-path pushes the event (or dispatches via UQM), but:
- **Option A** (surgical `notify_one`): `notify_for` returns `None`. Must spawn a
  worker. The fast-path spawn has no access to `worker_join_set_dispatch` (a
  `Arc<Mutex<JoinSet>>` local to the dispatch loop). A worker spawned inside the
  detached fast-path task would be untracked — it would not be joined on shutdown.
- **Option B** (UQM::dispatch in handler or runner): `dispatch` returns
  `WorkerSpawned`. The fast-path task must propagate this outcome to runner.rs's worker
  spawn block. The current fast-path `tokio::spawn` body does not return a value to the
  dispatch loop — it is detached.
- **Option C** (runner interception): runner.rs has direct access to `worker_join_set`
  and can spawn properly, but requires command parsing in the runner.

**Resolution options for B/C:** (a) The fast-path spawn calls `dispatch`, observes
`WorkerSpawned`, and spawns a worker inline — tracking it in a local `JoinSet` or
accepting that it is untracked (acceptable for short-lived command handlers per D-27
"commands are short-lived"). (b) The entire routing decision moves back to runner.rs
synchronously — the dispatch loop does a pre-check before the `tokio::spawn`.

### 3. Worker at queue depth = capacity (cap-hit path for `/queue`)

If the queue is at 128 events and the user sends `/queue more`:
- **Current behavior (bug present):** `try_push` returns `CapacityReached`,
  handler emits ❌ + "Queue is full" reply. Worker is not woken. Correct for the cap-hit
  case — no event was pushed, no wake needed.
- **Option B via `UQM::dispatch`:** `dispatch` calls `try_push`, gets
  `CapacityReached`, sends D-11 UX (❌ + chat reply from UQM), returns
  `Err(QueueError::CapacityReached)`. No `notify_one()` called. Correct.
- **Duplication hazard:** If Option B intercepts in runner.rs but the handler's `Queued`
  arm is still reachable from some other code path (e.g., the existing busy-branch in
  `session_queue_integration.rs`), and both paths emit the cap-hit reply, a user could
  receive two "Queue is full" messages. The planner must confirm which cap-hit path
  remains live.

### 4. Multimodal sidecar alignment

`UQM::dispatch` pushes a sidecar entry for each successful `try_push` via
`push_multimodal(&session_key, (text_prefix, image_data_uri))` (user_queue.rs:144).
The `/queue` command is text-only by contract (D-27). The fast-path already passes
`(None, None)` for both multimodal fields (runner.rs:949-952). If Option B routes
`/queue` events through `UQM::dispatch`, dispatch will call `push_multimodal` with
`(None, None)` — a no-op sidecar entry. The worker then calls `take_multimodal` and
gets `Some((None, None))`, which `unwrap_or((None, None))` normalizes to `(None, None)`.
No behavioral difference. **Not a hazard, but the planner should confirm the `Some((None, None))`
vs `None` distinction does not cause a FIFO skew in `take_multimodal`'s `VecDeque`.**

### 5. SessionKey construction in the handler vs. in runner.rs

The handler constructs `session_key` from `event.platform`, `event.chat_id`,
`event.sender_id` using the same `SessionKey::new(...).with_user(...)` pattern (D-14).
The runner constructs an identical key at `runner.rs:1001-1002` before the
`DispatchOutcome` match. If Option B's interception moves into runner.rs, the key is
already available at `runner.rs:1001` — no duplication risk. If it stays in handler.rs,
the handler builds the key at `handler.rs:946-948` — also correct. Both sites use the
full triple (D-14 invariant).

### 6. Cancellation token propagation

The fast-path spawn passes `cancel_cmd.child_token()` to `handle_with_multimodal`
(runner.rs:957). If `handle_with_multimodal` is replaced or bypassed (Option C), the
cancellation token must still be passed to `UQM::dispatch` — but `dispatch` is
synchronous w.r.t. cancellation (it does not hold a long-running task). The `dispatch`
call itself is fast; the spawned worker inherits cancellation via the
`cancel_task.cancelled()` branch in its `select!`. No new cancellation hazard.

### 7. Plan 05 fast-path interaction — `continue` prevents double-routing

The fast-path (runner.rs:970: `continue;`) ensures that a slash-command event never
reaches `UQM::dispatch` via the normal free-text path. If Option B adds a `UQM::dispatch`
call inside the fast-path spawn body (or runner.rs pre-spawn), and the `continue` remains,
there is no double-routing. If Option C moves the routing out of the fast-path entirely
(runner.rs intercepts before the `starts_with('/')` branch), the `continue` must be
preserved for all OTHER slash commands that still go through `handle_with_multimodal`.

### 8. `test_slash_command_bypasses_per_chat_worker` — assertion (b) must still pass

This test (uqm_session_queue_unification.rs:835) asserts that after
`handle_with_multimodal("/queue interrupt this")`, `session_queue.len(&session_key) == 2`
(the pre-loaded `free_1` plus the synthesized `/queue` event). The synthesized event has
`content == "interrupt this"`. This assertion depends on the handler's `Queued` arm
calling `session_queue.try_push`. Under Option B (UQM::dispatch in handler), the push
goes through `UQM::dispatch` → `session_queue.try_push` — same observable side-effect.
Under Option C (runner-level interception), `handle_with_multimodal` is never called for
`/queue`, so the test would fail as written. **Option C requires this test to be rewritten.**

---

## Test Surface

### Required regression test: `/queue` wakes idle worker

**Test name:** `test_queue_command_wakes_idle_worker`

**Harness shape** (pattern-matches `spawn_test_worker` in `uqm_session_queue_unification.rs`):

1. Build `RecordingFailingAdapter`, `SessionQueue`, `UserQueueManager`, handler, worker —
   same setup as `test_5_same_chat_messages_emit_5_eye_reactions_and_fifo_order_through_handler`.
2. Dispatch one free-text event via `uqm.dispatch(...)` to create the worker
   (`WorkerSpawned`). Call `spawn_test_worker(...)`.
3. Drain that first event: poll until `send_log.len() >= 1` (worker processed it and
   returned to `notified().await` park).
4. Now the worker is parked. Dispatch 5 `/queue <text>` events via
   `handler.handle_with_multimodal(make_event("/queue msg1"), ...)` directly — this
   exercises the exact code path that was broken (fast-path-equivalent call without going
   through `UQM::dispatch`).
5. Poll until `send_log.len() >= 6` (1 original + 5 from the `/queue` events). Timeout
   2 seconds.
6. Cancel, join worker.
7. **Assertions:**
   - `send_log.len() == 6` — all 5 queued events were processed.
   - `session_queue.len(&session_key) == 0` — no orphaned events.
   - `reactions` log has exactly 5 `👀` reactions for the 5 `/queue` synthesized events
     (emitted at pop-time by the worker, D-08 invariant).
   - FIFO order: `msg1` → `msg2` → `msg3` → `msg4` → `msg5` in reaction sequence.

**Why this test catches the regression:** Under the unfixed code, step 5 times out.
The 5 `/queue` events sit in SessionQueue with no wake — `send_log` never reaches 6.

### Required regression test: `/queue` on a fresh chat (no prior worker)

**Test name:** `test_queue_command_on_fresh_chat_spawns_worker`

**Harness shape:**

1. Build components as above but do NOT dispatch any free-text event first.
2. Dispatch 3 `/queue` events via `handle_with_multimodal` (no UQM path).
3. Assert SessionQueue depth == 3.
4. Observe whether `uqm.notify_for(&session_key)` returns `Some` or `None`.
   - If `None` (no worker registered), assert that a worker is spawned (however the
     fix implements that), and that all 3 events are eventually processed.
   - If `Some` (a worker was implicitly registered by the fix), assert it drains.
5. Timeout 2 seconds.

**Note:** This test may reveal whether the fix correctly handles the no-worker path.
The planner must decide whether the no-worker path is in scope for this fix phase.

### Existing tests that must continue to pass unchanged

- `test_slash_command_bypasses_per_chat_worker` (uqm_session_queue_unification.rs:835)
  — assertion (b) requires the handler to push the synthesized event via `try_push`.
  Options A and B preserve this. Option C breaks it.
- `test_cmd_queue_enqueues_and_replies_with_depth` (session_queue_integration.rs:569)
  — exercises the full `/queue` path through `MessageHandler::handle`. Must still pass.
- `test_cmd_queue_cap_hit_emits_reaction` (session_queue_integration.rs:643)
  — cap-hit on the `/queue` path. Must still pass regardless of which cap-hit reply
  fires (handler-side vs. UQM-side).
- All 6 `uqm_session_queue_unification.rs` tests.
- All 9 `session_queue_integration.rs` tests.

---

## Open Questions for the Planner

**Q1 — Option A vs. Option B vs. Option C?**

Given that Option A requires adding `Arc<UserQueueManager>` to `GatewayMessageHandler`
AND solving the no-worker spawn problem inside a detached task, while Option B requires
the same field addition but inherits UQM's correct push+notify+spawn-signal atomicity,
and Option C avoids touching `handler.rs` but breaks command parsing encapsulation and
requires rewriting `test_slash_command_bypasses_per_chat_worker`:

Should the fix use Option B (UQM::dispatch replaces try_push in handler's Queued arm)
with the `WorkerSpawned` outcome handled by having the fast-path spawn call `dispatch`
and then conditionally spawn a worker inline (untracked, acceptable per D-27), or should
it use a hybrid of Option B where the interception lives in runner.rs's fast-path spawn
before the `handle_with_multimodal` call?

**Q2 — Is the no-worker case (fresh-chat `/queue`) in scope for this fix?**

The UAT failure involved an existing chat with an active worker. The no-worker path
(first message to a chat is `/queue`) is a distinct scenario. Fixing it requires the
fast-path to propagate `WorkerSpawned` back to the dispatch loop. Should this phase fix
only the "wake existing parked worker" case (common case) and defer the "spawn worker
on `/queue` for fresh chat" case, or must both be fixed atomically?

**Q3 — Cap-hit reply deduplication after Option B?**

Under Option B, a `/queue` at depth 128 would fire the D-11 UX from inside
`UQM::dispatch` (❌ + "Queue is full"). The handler's existing `Queued` arm cap-hit
path (handler.rs:823-843) would become unreachable from the fast-path but would remain
live for the busy-branch (direct handler calls from `session_queue_integration.rs`
tests). Is a single cap-hit reply source acceptable, or must both paths be aligned?

**Q4 — Depth-aware reply under Option B?**

After `UQM::dispatch` succeeds (returns `Ok(Accepted | WorkerSpawned)`), the synthesized
event is on the queue. The depth reply ("Queued for the next turn. (N queued)") requires
`session_queue.len(&session_key)` after the push. Under Option B, if dispatch happens
inside the handler's Queued arm, `self.session_queue.as_ref().map(|q| q.len(&key))`
is still accessible. If dispatch is moved to runner.rs, the reply must be sent from
runner.rs. Which location is preferred?

**Q5 — Test harness: should `build_test_handler_with_queue` also accept `Arc<UserQueueManager>`?**

The existing test helper `build_test_handler_with_queue` in both test files wires only
`Arc<SessionQueue>`. If Options A or B add `Arc<UserQueueManager>` to
`GatewayMessageHandler`, a new `build_test_handler_with_queue_and_uqm` helper (or an
extended signature) is needed. Should the planner add a combined setter
`handler.set_queue_and_uqm(queue, uqm)` or two separate setters? The combined setter
enforces that they share the same underlying `Arc<SessionQueue>` (invariant: UQM and
handler must reference the same queue instance to avoid FIFO splits).

---

## Sources

All findings are from direct source inspection of the live codebase. No external
references needed — the root cause is a missing function call identifiable by grep.

- `crates/ironhermes-gateway/src/handler.rs:799-861` — buggy `/queue` Queued arm
- `crates/ironhermes-gateway/src/handler.rs:154` — `session_queue: Option<Arc<SessionQueue>>` field (no UQM)
- `crates/ironhermes-gateway/src/user_queue.rs:100-165` — `UQM::dispatch` including the sole `notify_one()` at line 154
- `crates/ironhermes-gateway/src/runner.rs:935-970` — slash-command fast-path (D-23/D-27)
- `crates/ironhermes-gateway/src/runner.rs:1039-1046` — worker pop-loop parking on `notify_task.notified()`
- `crates/ironhermes-gateway/src/session_queue.rs:111-134` — `try_push` (no Notify, no wake semantics)
- `crates/ironhermes-gateway/tests/uqm_session_queue_unification.rs` — existing test surface
- `crates/ironhermes-gateway/tests/session_queue_integration.rs` — existing test surface
- `.planning/phases/36.17.2-unify-session-queue-replace-uqm-mpsc-buffer/36.17.2-CONTEXT.md` — D-01..D-28 locked decisions
- `.planning/phases/36.17.2-unify-session-queue-replace-uqm-mpsc-buffer/36.17.2-VERIFICATION.md` — post-rewrite state confirmation
