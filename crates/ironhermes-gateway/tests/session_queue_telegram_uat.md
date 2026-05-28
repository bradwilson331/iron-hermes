# Manual UAT: Session Queue — Telegram Live UAT

> **Phase reference:** Phase 36.17.2 (supersedes 36.17.1-05)
> `.planning/phases/36.17.2-unify-session-queue-replace-uqm-mpsc-buffer/`
>
> **Architecture (locked):** UnifiedQueueing (Option C, D-01..D-22). UserQueueManager's per-chat mpsc buffer has been removed; SessionQueue is the single source of truth for buffered messages. 👁 transport reactions now fire AT POP TIME (the moment the worker begins processing each message), NOT at dispatch time (when the message lands on the queue). This is the only user-visible change from 36.17.1.
>
> **Locked decisions exercised here:**
> - **D-01** — full `/queue` feature parity (queue type + gateway wiring + `/queue` + busy-agent enqueue + `/new`+`/reset` clearing + drain-mode).
> - **D-02** — Telegram is the **only** wired platform in this phase. Discord, Slack, and `iron_hermes_ui` web wiring are deferred to follow-up phases.
> - **D-03** (36.17.1) — drain-mode preservation is **in-process only**. Cross-process restart preservation is out of scope (see §"Out of Scope" below).
> - **D-04** — Per-chat worker loop rewrites to poll `SessionQueue::pop` directly (Notify-based idle wait).
> - **D-08** — 👁 transport reaction emission moves from `dispatch` to worker (fires at pop time, immediately before each `handle_with_multimodal` call).
> - **D-11** — Cap-hit UX lives in `UserQueueManager::dispatch`. When `try_push` returns `CapacityReached`, UQM fires the ❌ reaction + chat reply directly; the Telegram dispatch loop does not need to handle it.
> - **D-13** — Telegram cap-hit UX: `❌` reaction via `PlatformAdapter::add_reaction`, then a chat reply `⏳ Queue is full (128 messages). Wait for the agent to drain before sending more.`
> - **D-22** — Live Telegram UAT runbook gated as a `checkpoint:human-verify` task.
>
> **Why this runbook exists:** the automated suite in
> `tests/session_queue_integration.rs` and `tests/uqm_session_queue_unification.rs`
> covers the in-process handler + drain paths exhaustively. What automated tests
> **cannot** verify is:
>
> 1. Whether Telegram actually renders the `❌` reaction on the dropped 129th
>    message in the user's client.
> 2. Whether Telegram's `getUpdates` re-delivers the dropped 129th update
>    (the runner-loop offset advance at `runner.rs:723-736` is supposed to
>    prevent this — see **Pitfall 6** below).
> 3. Whether the user-visible chat reply `⏳ Queue is full (128 messages).
>    Wait for the agent to drain before sending more.` displays correctly.
> 4. Whether the **Plan 04 in-process drain-mode flag** (`is_draining`) fires
>    on a real `ctrl+c` / `SIGTERM` and visibly logs `is_draining=true` BEFORE
>    the cancel-fired line.
> 5. Whether 👁 reactions appear SEQUENTIALLY (one per message at pop time)
>    rather than as a burst at dispatch time (D-08 regression detection).
>
> All five require a live Telegram bot, a live IRONHERMES_HOME, and visual
> inspection of the chat.

---

## Prerequisites

Before running any scenario:

| Item | Value | How to set |
|------|-------|------------|
| `TELEGRAM_BOT_TOKEN` | Your test bot's token from `@BotFather` | `export TELEGRAM_BOT_TOKEN=...` |
| `IRONHERMES_HOME` | Test config dir — separate from your production home | `export IRONHERMES_HOME=/tmp/uat-36.17.2-home` |
| Test chat | A Telegram chat where the bot is admin'd; the bot must respond to free-text | Add bot, `/start`, send "hi" to confirm |
| Agent config | Free-text messages must produce a real agent turn (so we can observe busy-vs-idle) | Pick a model in `cli-config.yaml`; verify the bot responds to "hi" with a real LLM reply |
| Phase 36.17.2 plans 01–04 merged | Required — earlier plans rewrite UQM internals, the per-chat worker pop-loop, and the integration tests | `git log --oneline | grep 36.17.2` |

Start the gateway in a terminal you can `ctrl+c` later for Scenario 4b:

```bash
cargo run --release --bin ironhermes -- gateway 2>&1 | tee /tmp/uat-36.17.2-gateway.log
```

Keep this terminal visible — Scenarios 3 and 4 read the log to verify the
`tracing::warn` cap-hit line and the `is_draining=true` shutdown line.

---

## Scenario 1: Busy-enqueue silent path

**What this verifies (D-01b + D-08 + D-11 + Pitfall 1):** while the per-session
agent is mid-turn, an incoming free-text Telegram message is enqueued onto
the `SessionQueue` and replays automatically after the current turn ends.
No extra "queued!" chat reply appears. Under Phase 36.17.2, the 👁 transport
reaction fires AT POP TIME (when the worker begins each turn), NOT at
dispatch time. This scenario verifies the D-08 timing semantics.

**Steps:**

1. In your test chat, send the following 5 messages in **rapid succession**
   (less than 2 seconds between each — fast enough to enqueue messages 2..5
   while the agent is still processing message 1):

   - Message 1: `"Write 1500 words about Soviet-era radio engineering, with citations."`
   - Message 2: `"And include a paragraph about Popov."`
   - Message 3: `"And a section on Lissajous figures in oscilloscopes."`
   - Message 4: `"And a closing paragraph on wartime jamming techniques."`
   - Message 5: `"And summarize with exactly three bullet points."`

2. After sending all 5, observe the Telegram chat carefully.

   - Watch for 👁 reactions appearing on each message. Note the TIMING:
     does 👁 appear on messages 2..5 immediately after sending (dispatch
     time), or later when each message's turn actually begins (pop time)?
   - Under Phase 36.17.2 (unified architecture), 👁 appears at pop time —
     NOT at dispatch time.

3. Wait for all 5 agent turns to complete.

**Expected (Phase 36.17.2 unified architecture):**

1. Message 1 starts processing immediately. You may see a 👁 reaction appear on message 1 BEFORE the agent begins (this is the worker emitting 👁 at the moment of pop — D-06 step 3, D-08). Note: the legacy 36.17.1 behavior emitted 👁 only on messages 2..N. Under 36.17.2, every message gets 👁 at the moment its turn begins, INCLUDING the first one.
2. Messages 2..5 sit silently in the SessionQueue while message 1 is processed. NO 👁 reactions appear on messages 2..5 yet — they have not been popped.
3. When message 1's agent turn completes, the worker pops message 2. A 👁 reaction appears on message 2 at THIS moment (not earlier). The agent begins processing message 2.
4. Repeat for messages 3, 4, 5 — each gets a 👁 reaction at the moment of pop, sequentially.
5. Final state: all 5 messages have been processed in FIFO order; each has a 👁 reaction; the SessionQueue depth for this session is 0.

- **No** "Queued for the next turn." chat reply is sent for messages 2..5 — free-text enqueue is silent per D-13.
- `/tmp/uat-36.17.2-gateway.log` shows sequential pop events (not a dispatch-time burst).

**Regression signal:** If 👁 reactions appear on messages 2..5 BEFORE message 1's turn completes, the architecture has regressed — 👁 emission has moved back to dispatch (violating D-08). File a bug citing T-36.17.2-03 regression and DO NOT sign off on this scenario.

**Architectural justification:** Under 36.17.1, 👁 emission was a dispatch-time signal ("UserQueueManager has accepted your message into its buffer"). Under 36.17.2, 👁 is a processing-start signal ("the agent has begun working on this specific message"). The new semantics align with user mental models: when 👁 appears, the agent is actually looking at the message.

---

## Scenario 2: `/queue` command path

**Note (36.17.2):** /queue continues to write directly to SessionQueue via the gateway's handle_slash_command intercept. UQM::dispatch is bypassed for /queue (slash commands are not whitelist/dispatch-path messages). This scenario verifies the slash-command path is unaffected by the dispatch-path refactor.

**What this verifies (D-08 + Plan 03):** the `/queue <text>` slash command
unconditionally pushes onto the SessionQueue (regardless of agent_running
state) and responds with a depth-aware confirmation reply.

**Steps:**

1. Make sure no agent turn is in flight (wait for any prior conversation
   to finish).

2. Send: `/queue do this next`

3. Send: `/queue and then this`

4. Send: `/queue and finally this`

**Expected:**

- 1st `/queue do this next` → bot replies `Queued for the next turn.`
  (depth == 1, singular form).
- 2nd `/queue and then this` → bot replies `Queued for the next turn. (2
  queued)` (depth == 2, plural form with count).
- 3rd `/queue and finally this` → bot replies `Queued for the next turn.
  (3 queued)` (depth == 3).

The queued items will NOT auto-replay until the next free-text agent turn
runs (per Plan 02 drain wiring — the drain helper fires after the next
`run_agent` completion). Send a free-text prompt to trigger drain:

5. Send: `Status update on the queued items?`

**Expected continued:**

- The free-text message starts an agent turn. After that turn completes,
  the three queued items replay one-by-one in arrival order (each as a
  fresh `run_agent` invocation — no merging).

---

## Scenario 3: Cap-hit UX (D-13 — Telegram-specific)

**Note (36.17.2 internal migration):** The cap-hit UX literal (❌ reaction + "⏳ Queue is full (128 messages). Wait for the agent to drain before sending more." chat reply) now fires from inside UserQueueManager::dispatch (D-11). User-visible behavior is identical to 36.17.1. If you see the cap-hit reply for messages that should NOT have hit cap (e.g., depth was <128), file a bug — the migration may have moved the emission to the wrong layer.

**What this verifies (D-13 + T-36.17.1-01 + Pitfall 6):** when the queue
hits the 128-message hard cap, the 129th attempt receives a `❌` reaction
and a `⏳ Queue is full (128 messages). Wait for the agent to drain before
sending more.` chat reply. The cap MUST hold at 128 (no overflow). Telegram
MUST NOT re-deliver the dropped 129th update on the next `getUpdates` poll
(this is the **Pitfall 6** invariant — `runner.rs:723-736` advances the
offset BEFORE dispatch).

**Setup — start a slow turn so the queue can fill while the agent is busy:**

1. Send a long-running prompt:

   ```
   Research the history of FIFO queues from von Neumann onward and write 3000 words with citations.
   ```

   Wait until the agent begins streaming.

2. While the agent is still generating, flood the queue with 128 `/queue`
   commands. **Tooling depends on your setup — the runbook documents
   intent, not a fixed command.** Examples:

   - If you have a Telegram CLI client (e.g. `tdl`, `telegram-cli`, the
     Bot API directly, or a small Python `python-telegram-bot` script):

     ```bash
     for i in $(seq 1 128); do
       # Adapt to your tooling — the literal "/queue msg $i" is what
       # matters; the transport mechanism is up to you.
       telegram-cli msg @your_test_bot "/queue msg $i"
       sleep 0.05
     done
     ```

   - Or, simpler if you have curl + the Bot API token:

     ```bash
     for i in $(seq 1 128); do
       curl -s -X POST \
         "https://api.telegram.org/bot$TELEGRAM_BOT_TOKEN/sendMessage" \
         -d "chat_id=<YOUR_CHAT_ID>&text=/queue msg $i" \
         > /dev/null
       sleep 0.05
     done
     ```

   The sleep is to stay under Telegram's per-chat ~20/sec rate limit.

3. Send the 129th: `/queue msg 129` (a single manual message is fine —
   you want to be able to see exactly which message gets the ❌).

**Expected:**

- The 129th message receives a `❌` reaction (visible in your Telegram
  client within ~1s of sending).
- The bot replies in the chat with the literal text: `⏳ Queue is full
  (128 messages). Wait for the agent to drain before sending more.`
- The currently-streaming "history of FIFO queues" turn continues normally
  — cap-hit MUST NOT interrupt the in-flight turn.
- `/tmp/uat-36.17.2-gateway.log` shows a `SessionQueue: capacity reached,
  message dropped` warn line tagged with the session key.

4. **Pitfall 6 verification — no re-delivery:**

   Wait ~30 seconds. In your Telegram client, observe whether the 129th
   message receives ANY additional reactions or processing. Also check
   `/tmp/uat-36.17.2-gateway.log` for any duplicate `msg_id 129`
   processing:

   ```bash
   grep "msg 129\|msg_129\|<id_you_used_for_129>" /tmp/uat-36.17.2-gateway.log
   ```

   **Expected:** exactly one cap-hit warn line for the 129th message; no
   duplicate processing. This proves the Telegram `getUpdates` offset
   advance at `runner.rs:723-736` fires BEFORE the handler-level cap-hit
   UX, so Telegram considers the 129th update consumed even though the
   gateway dropped it.

   If you observe re-delivery (duplicate ❌ reactions, duplicate warn
   lines, infinite retry from Telegram's side), Pitfall 6 has regressed
   — capture the log lines and reject sign-off.

5. **Drain verification (slow):** wait until all 128 queued items drain
   (the agent processes them one-by-one, each as a full turn — this may
   take minutes depending on your model + prompt complexity). Verify no
   "extras" appear (the 129th must NOT replay since it was dropped).

---

## Scenario 4: `/new` clears queue + Plan 04 drain-mode preservation

This scenario has two subsections — both must pass to sign off the
phase. Run them in order; reset state between them.

### Scenario 4a: `/new` clears queue (Pitfall 5)

**Note (36.17.2):** /new behavior is unchanged. The clear-queue-before-remove ordering is preserved from 36.17.1-03 (handler.rs NewSession arm).

**What this verifies (Plan 03 Task 2 + Pitfall 5):** running `/new` while
the queue has items must CLEAR the queue BEFORE removing the session.
The queued items MUST NOT replay after `/new`.

**Steps:**

1. Make sure no agent turn is in flight.

2. Send `/queue msg A`, `/queue msg B`, `/queue msg C` — three quick
   `/queue` commands.

3. Verify (visually) the bot's replies show depth growing: 1 → 2 → 3.

4. Send `/new`.

5. Send a free-text trigger to flush any pending drain: `hi`.

**Expected:**

- After `/new`, the bot replies `Conversation cleared. Starting fresh.`
- The `hi` message produces a single fresh agent turn — none of the
  three queued items (msg A / msg B / msg C) replay.
- `/tmp/uat-36.17.2-gateway.log` shows session removal followed by no
  further drain activity for that session_key.

If any of the three queued items DO replay after `/new`, Pitfall 5 has
regressed — reject sign-off.

### Scenario 4b: Plan 04 in-process drain-mode flag (`is_draining`)

**What this verifies (D-03 + T-36.17.1-03 + Plan 04):** the gateway's
graceful shutdown sequence calls `self.drain_for_restart()` which sets
`is_draining=true` **BEFORE** firing `self.cancel.cancel()`. Messages
arriving in the brief drain-mode window (between flag flip and process
exit) are preserved on the session queue rather than dropped.

**Out of scope (intentional):** cross-process restart preservation. The
queue is in-memory; `kill -9 ironhermes-gateway` LOSES queue contents
by design. See §"Out of Scope" below.

**Steps:**

1. Start a slow agent turn:

   ```
   Compose a 2000-word essay on Tarkovsky's use of time.
   ```

2. While the agent is streaming, queue a couple of items so we can
   confirm they survive into the drain window:

   ```
   /queue follow-up A
   /queue follow-up B
   ```

3. Verify the bot acknowledges both (depth 1, then 2). The agent should
   still be streaming the Tarkovsky essay.

4. In the gateway terminal (where you ran `cargo run … gateway`), press
   `ctrl+c` ONCE. **Do not press it a second time** — that triggers a
   forced abort, not a graceful drain.

5. Watch the gateway log carefully. You are looking for a specific line
   ordering. Filter:

   ```bash
   grep -E "is_draining|drain_for_restart|cancel|Shutting down" \
     /tmp/uat-36.17.2-gateway.log | tail -20
   ```

**Expected:**

- A log line indicating `drain_for_restart` was called (or equivalently
  `is_draining` was set to true) BEFORE any "cancel fired" / "cancellation
  propagated" line. The Phase 36.17.1 Plan 04 source order is:

  ```rust
  self.is_draining.store(true, Ordering::SeqCst);
  self.cancel.cancel();
  ```

  Phase 36.17.1 Plan 04's `drain_for_restart_stores_flag_before_cancel`
  unit test locks this ordering at source-grep level. The live UAT
  verifies it is also observable at runtime.

- If the gateway emits a `tracing::info!` / `warn!` that mentions
  draining, capture the timestamp. Then capture the timestamp of the
  next `cancel` / "Propagating cancellation" line. The draining line
  MUST have an earlier (or equal — sub-millisecond) timestamp.

- Verify that any free-text message sent in the ~0.5s window between
  the `ctrl+c` and process exit is queued onto session_queue (NOT
  dropped). In practice this window is tiny — for visual verification
  it is sufficient to confirm the `is_draining=true` log line appears
  AT ALL before exit. **In-process only**: once the process exits, queue
  contents are gone (see §"Out of Scope").

---

### Note on D-12 — HTTP 429 (deferred)

CONTEXT.md D-12 specifies that HTTP-arrival platforms (future webhook/REST adapter, Discord/Slack when they flow through `UserQueueManager`) inherit the `Err(QueueError::CapacityReached)` signal at the dispatch boundary and respond with **HTTP 429 + `Retry-After: 5`** at the network edge.

**Status in 36.17.2:** DEFERRED. Phase 36.17.2 only ships the Telegram path; the webhook/REST adapter (Phase 36.7.1+) does not yet flow through `UserQueueManager::dispatch`. The 429 path is wired ARCHITECTURALLY (the `Err(QueueError::CapacityReached)` return shape is in place) but no UAT scenario can exercise it without the adapter.

**Verifier action:** None. This note exists so D-12 is not an orphan decision in the UAT sign-off — it is captured here as a deliberate future-phase deferral.

---

## Scenario 5 — Worker-exit/dispatch race (T-36.17.2-01)

**Goal:** Verify that after a per-chat worker exits (queue drained to empty, worker calls UQM::remove and dies), a subsequent dispatch correctly spawns a fresh worker. This validates the mutex serialization in UserQueueManager between `dispatch` and `remove` (D-19 + T-36.17.2-01 mitigation).

**Verifier steps:**

1. Send a single message: `"first message after race test"`. Wait for the agent to fully complete its turn — confirm the agent has fully responded AND the 👁 reaction is visible AND no further activity is happening (look for the "agent finished" log line `tail -f` on the gateway logs, or just wait ~30 seconds after the agent's last visible reply).

2. At this point, the per-chat worker for your SessionKey has exited (it popped the queue, found it empty, hit the cancel/notify select arm with no pending notification, and dropped through to the `queue_task.remove(&session_key_task).await` call at the bottom of the worker spawn closure).

3. Wait at least 5 seconds AFTER the agent's final reply. This guarantees the worker has fully exited and released its Notify clone.

4. Send a second message: `"second message after race test"`.

5. Confirm: the second message is processed normally. A 👁 reaction appears on it. The agent responds.

**Expected (Phase 36.17.2):**

- The second message MUST be processed. If it sits in the queue indefinitely with no 👁 reaction, the dispatch-after-worker-exit path is broken (T-36.17.2-01 mitigation has regressed).
- gateway logs MUST show TWO instances of "Per-chat worker exited" or equivalent debug line — one after each message's turn.
- Each message MUST receive exactly one 👁 reaction (no duplicates from the race).

**Regression signal:** If the second message is silently dropped, file T-36.17.2-01 regression. Likely causes: UQM::dispatch's lock re-check (after acquiring `workers.lock().await`) is failing to re-evaluate the map state correctly, OR the Arc<Notify> dropped before `dispatch`'s notify_one call.

---

## Scenario 6 — Multimodal payload preservation (M1 sidecar live evidence, T-36.17.2-04)

**Goal:** Verify that the multimodal sidecar (`pending_multimodal` introduced in Plan 01) preserves `(text_prefix, image_data_uri)` payloads in FIFO lockstep with `SessionQueue`, so a burst of mixed photo/document/text messages reaches `handle_with_multimodal` with the correct attachments per message.

**Background:** 36.17.1 stored multimodal payload inside `QueuedMessage` on the mpsc channel. 36.17.2 removes the mpsc and stores `MessageEvent` only in `SessionQueue`; multimodal payload moves to a separate `pending_multimodal` map indexed by `SessionKey` with `VecDeque` FIFO semantics. If the sidecar drifts out of lockstep with `SessionQueue` (e.g., a push to the queue with no matching `push_multimodal`, or two takes per pop), photos arrive misaligned to their captions and the agent sees the wrong attachment for each turn. This scenario is the live evidence that the lockstep holds.

**Verifier steps:**

1. Wait until the agent is idle for the test chat (no pending replies, queue depth 0).

2. Send the following 5 messages in **rapid succession** (< 2 seconds between each — fast enough to enqueue messages 2..5 while the agent is still processing message 1):
   - Message 1: text only, content `"first — text only"`.
   - Message 2: a photo attachment (any JPEG/PNG, ≤ 1MB), with caption `"second — photo"`.
   - Message 3: a document attachment (any PDF/txt, ≤ 1MB), with caption `"third — document"`.
   - Message 4: a photo with caption `"fourth — photo with caption B"`.
   - Message 5: text only, content `"fifth — text only"`.

3. Wait for the agent to process all 5 messages (~30-90 seconds depending on response length). Observe the 👁 reactions appear sequentially at pop time (Scenario 1 behavior).

4. Inspect each agent response in Telegram. For each, verify the agent's response references the CORRECT attachment for that turn — message 2's response talks about the photo, message 3's response talks about the document, etc.

**Expected (Phase 36.17.2):**

- 5 messages processed in FIFO order.
- 5 👁 reactions at pop time (one per message, Scenario 1 behavior).
- Each agent response correctly references its message's attachment (or lack thereof for text-only messages).
- Message 1 and Message 5 (text-only) receive responses that do NOT reference any image/document.
- Message 2 and Message 4 (photo) receive responses that reference the image content (e.g., describe what the photo shows).
- Message 3 (document) receives a response that references the document content.

**Regression signal:** If message 3's response talks about the photo (instead of the document), the sidecar has drifted by one position. If message 1 (text-only) receives a response describing a phantom image, the sidecar's pop returned a stale payload from a previous turn. Either case indicates `take_multimodal`/`push_multimodal` lockstep with `SessionQueue::try_push`/`SessionQueue::pop` has broken — file T-36.17.2-04 regression and DO NOT sign off on this scenario.

**Note:** This scenario does NOT require the cap to be hit. It exercises sidecar ordering across the busy-enqueue path, which is the dispatch path the architectural shift made reachable. If Plan 03's `test_multimodal_payload_roundtrips_through_sidecar_to_handler` integration test passes, this live scenario should also pass — Scenario 6 is the human-side confirmation that automation + production behave consistently.

---

## Scenario 7 — Slash-command fast-path during busy turn (D-23, D-27, T-36.17.2-06)

**Goal:** Verify that slash commands dispatched while the per-chat worker is mid-turn on a free-text event respond in sub-second wall-clock time, NOT after the in-flight turn completes. This is the second UAT failure mode surfaced after 36.17.2 Plans 01-04 shipped — Plans 01+02 unified free-text bursts, but slash commands were still serializing behind the worker until Plan 05 added the dispatch-loop fast-path.

**Background:** Under 36.17.1 (and the post-Plan-02 / pre-Plan-05 state of 36.17.2), `/queue` typed during a long-running free-text turn waited for the in-flight `handle_with_multimodal` to return (~90s observed in UAT). Plan 05 (D-27) added a dispatch-loop branch at `runner.rs:~940` that bypasses `UserQueueManager::dispatch` for events where `event.content.starts_with('/')`. The fast-path `tokio::spawn`s the handler call directly, acquiring the per-chat `sem_dispatch` permit (T-36.17.2-06 mitigation — prevents command-storm bypass amplification) but skipping the SessionQueue / worker pop-loop entirely.

**Verifier steps:**

1. Send a free-text message that triggers a long agent turn (≥ 15 seconds). Example: `"Write a 500-word essay about Rust async runtime architecture."` Confirm 👁 appears at pop time and the agent begins streaming.

2. While the agent is mid-stream (within the first 5-10 seconds, BEFORE the agent completes its turn), send `/queue some text for the next turn`.

3. Observe Telegram. The bot MUST reply to `/queue` with `"Queued for the next turn."` (or `"Queued for the next turn. (N queued)"` depending on existing queue depth) within **~1 second** of sending the slash command — well before the in-flight free-text turn completes.

4. Wait for the in-flight free-text turn to fully complete (agent finishes streaming, presumably 15+ more seconds).

5. Confirm the `/queue` synthesized event ("some text for the next turn") is then processed AS THE NEXT FREE-TEXT TURN — the worker pops it from SessionQueue, emits 👁, and the agent processes it.

**Expected (Phase 36.17.2 Plan 05):**

- `/queue` reply appears within ~1 second of dispatch (sub-second is ideal; up to 2 seconds tolerable for network jitter).
- The reply text is the depth-aware confirmation from cmd_queue (singular at depth 1, plural with `(N queued)` at depth > 1).
- The in-flight free-text turn continues uninterrupted — no streaming corruption, no abrupt cancellation.
- After the free-text turn finishes, the worker pops the `/queue` synthesized event and processes it as a normal turn.

**Regression signal:** If bot waits the full duration of the in-flight turn before replying to `/queue`, the fast-path is not wired. File a 36.17.2 Plan 05 regression and DO NOT sign off on this scenario. Likely causes: (i) the `event.content.starts_with('/')` branch in `runner.rs` is positioned AFTER `user_queue_dispatch.dispatch(...)` instead of BEFORE; (ii) the branch's `continue;` is missing, falling through to the multimodal+UQM path; (iii) the spawn forgot to call `handle_with_multimodal` and instead called something that goes through the worker (e.g., `runner.try_enqueue`).

**Bonus check (T-36.17.2-06 storm-bypass):** Optionally, type 10 slash commands in quick succession (e.g., `/help` × 10). Each command should respond, but if all 10 complete simultaneously with no observable serial delay (replies all arriving in the same instant with no semaphore-imposed staggering), the permit acquisition is suspect — investigate `sem_cmd.acquire().await` in the fast-path. The Task 1 grep gate (`grep -c "sem_cmd.acquire\|semaphore_dispatch.acquire"`) is the primary static evidence; this UAT bonus is a sanity check, not a hard pass/fail since the underlying semaphore capacity is typically 4-8 and 10 sub-second commands may legitimately fit within bursts.

---

## Out of Scope — Cross-Process Drain Preservation

Phase 36.17.1 is titled **"in-mem-fifo-queuing"**. The queue is held in
`Mutex<HashMap<SessionKey, VecDeque<MessageEvent>>>` on `GatewayRunner`;
**queue contents do not survive process exit.**

Specifically:

- `kill -9 ironhermes-gateway` (SIGKILL) → queue is lost. Expected by design.
- Graceful `ctrl+c` / SIGTERM → `is_draining=true` window briefly preserves
  in-flight `msg_rx` events into the queue, but the queue itself does NOT
  survive the process exit that follows.
- Re-running `cargo run … gateway` after a previous instance exited →
  starts with empty queues. Messages that arrived between the previous
  instance's `ctrl+c` and the new instance's startup are lost.

This is consistent with the hermes-agent Python reference
(`gateway/run.py:2298-2302`) — `_queue_during_drain_enabled` only
preserves events for the **current** Python process's restart-window.

Cross-process queue persistence (e.g. backing the queue with rusqlite or
a journal file) is **explicitly out of scope** for this phase. See the
research document:

- `.planning/phases/36.17.1-in-mem-fifo-queuing-parity-of-python-deque-for-chat-sessions/36.17.1-RESEARCH.md`
  §`§2298-2302` discussion and §"Assumptions Log" assumption A3.

If a future phase ships cross-process queue persistence, it will introduce
a new UAT scenario covering it. **Do NOT attempt to verify cross-process
preservation as part of this phase's sign-off.**

---

## Sign-off Checklist

Run each scenario once on a live Telegram bot. Tick when verified.

- [ ] **Scenario 1** — Busy-enqueue silent path (36.17.2 unified): 👁 reactions appear sequentially at pop time, not at dispatch time. Second through fifth messages receive 👁 only when each turn begins. No extra chat reply for queued messages.
- [ ] **Scenario 2** — `/queue` depth-aware reply: singular form at
      depth 1, plural with count at depth ≥ 2; drain fires on next
      free-text turn.
- [ ] **Scenario 3** — Cap-hit UX: 129th gets `❌` reaction + `⏳ Queue
      is full (128 messages). Wait for the agent to drain before sending
      more.` chat reply, cap held at 128. Pitfall 6 verified — no
      Telegram re-delivery of the 129th update.
- [ ] **Scenario 4a** — `/new` clears queue: queued items do NOT replay
      after `/new`. Pitfall 5 verified.
- [ ] **Scenario 4b** — Plan 04 `is_draining`: `ctrl+c` triggers a
      log line indicating drain-mode entry BEFORE the cancel-propagation
      line. In-process only — cross-process preservation NOT tested
      (see §"Out of Scope").
- [ ] **Scenario 5** — Worker-exit/dispatch race (T-36.17.2-01): second-message-after-worker-exit processed normally. Two worker-exit log lines visible. No silently dropped messages.
- [ ] **Scenario 6** — Multimodal payload preservation (T-36.17.2-04): photo/document burst preserves payload→message alignment FIFO. Message 3's response references the document (not the photo from message 2).
- [ ] **Scenario 7** — Slash-command fast-path during busy turn (T-36.17.2-06): /queue replies in ~1s while free-text turn is in-flight; in-flight turn completes uninterrupted; synthesized /queue event processes next

Verifier (your name): ____________________

Date (YYYY-MM-DD): ____________________

Notes / failures (attach screenshots + log excerpts if any scenario fails):

```
[paste here]
```

---

## Document History

- **2026-05-27 (Phase 36.17.2-05):** Added Scenario 7 (slash-command fast-path during busy turn) closing T-36.17.2-06 storm-bypass and D-23/D-27 fast-path live evidence. Existing scenarios unchanged.
- **2026-05-27 (Phase 36.17.2-04):** Updated for unified architecture (D-01..D-22). Scenario 1's expected behavior rewrote — 👁 reactions now emit at pop time per D-08. Added Scenario 5 covering T-36.17.2-01 worker-exit/dispatch race. Added Scenario 6 covering T-36.17.2-04 multimodal sidecar lockstep (M1 sidecar live evidence). Added D-12 (HTTP 429) deferred-decision footnote. Other scenarios unchanged.
- **2026-05-27 (Phase 36.17.1-05):** Initial runbook for the mpsc-buffer architecture.

---

*Phase: 36.17.2 — unify-session-queue-replace-uqm-mpsc-buffer (supersedes 36.17.1-05)*
*Runbook updated: 2026-05-27*
