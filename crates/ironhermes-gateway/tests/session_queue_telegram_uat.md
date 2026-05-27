# Manual UAT: Phase 36.17.1 — in-mem FIFO Queuing (Telegram)

> **Phase reference:** `.planning/phases/36.17.1-in-mem-fifo-queuing-parity-of-python-deque-for-chat-sessions/`
>
> **Locked decisions exercised here:**
> - **D-01** — full `/queue` feature parity (queue type + gateway wiring + `/queue` + busy-agent enqueue + `/new`+`/reset` clearing + drain-mode).
> - **D-02** — Telegram is the **only** wired platform in this phase. Discord, Slack, and `iron_hermes_ui` web wiring are deferred to follow-up phases.
> - **D-03** — drain-mode preservation is **in-process only**. Cross-process restart preservation is out of scope (see §"Out of Scope" below).
> - **D-13** — Telegram cap-hit UX: `❌` reaction via `PlatformAdapter::add_reaction`, then a chat reply `⏳ Queue is full (128 messages). Wait for the agent to drain before sending more.`
>
> **Why this runbook exists:** the automated suite in
> `tests/session_queue_integration.rs` covers the in-process handler + drain
> paths exhaustively. What automated tests **cannot** verify is:
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
>
> All four require a live Telegram bot, a live IRONHERMES_HOME, and visual
> inspection of the chat.

---

## Prerequisites

Before running any scenario:

| Item | Value | How to set |
|------|-------|------------|
| `TELEGRAM_BOT_TOKEN` | Your test bot's token from `@BotFather` | `export TELEGRAM_BOT_TOKEN=...` |
| `IRONHERMES_HOME` | Test config dir — separate from your production home | `export IRONHERMES_HOME=/tmp/uat-36.17.1-home` |
| Test chat | A Telegram chat where the bot is admin'd; the bot must respond to free-text | Add bot, `/start`, send "hi" to confirm |
| Agent config | Free-text messages must produce a real agent turn (so we can observe busy-vs-idle) | Pick a model in `cli-config.yaml`; verify the bot responds to "hi" with a real LLM reply |
| Phase 36.17.1 plans 01–05 merged on `develop` | Required — earlier plans ship the SessionQueue, handler wiring, /queue intercept, /new clearing, drain-mode flag, and tests | `git log --oneline | grep 36.17.1` |

Start the gateway in a terminal you can `ctrl+c` later for Scenario 4b:

```bash
cargo run --release --bin ironhermes -- gateway 2>&1 | tee /tmp/uat-36.17.1-gateway.log
```

Keep this terminal visible — Scenarios 3 and 4 read the log to verify the
`tracing::warn` cap-hit line and the `is_draining=true` shutdown line.

---

## Scenario 1: Busy-enqueue silent path

**What this verifies (D-01b + D-13 + Pitfall 1):** while the per-session
agent is mid-turn, an incoming free-text Telegram message is enqueued onto
the `SessionQueue` and replays automatically after the current turn ends.
No extra "queued!" chat reply appears — the only visible signal is the
existing UserQueueManager 👁 transport-layer reaction.

**Steps:**

1. In your test chat, send a slow free-text prompt:

   ```
   Write 1500 words about Soviet-era radio engineering, with citations.
   ```

   Wait for the agent to begin streaming a reply (you should see typing
   indicators or partial output).

2. While the agent is still generating, send a follow-up free-text message
   (no slash command):

   ```
   And include a paragraph about Popov.
   ```

3. Observe the 👁 (eye) reaction appear on the second message within ~1s.
   This is `UserQueueManager`'s transport-layer signal — it fires for any
   message queued behind an in-flight per-chat worker run.

4. Wait for the first turn to finish (the "Write 1500 words…" reply
   completes).

5. The second message ("And include a paragraph about Popov.") replays
   automatically — the agent runs a fresh turn for it without you having
   to resend.

**Expected:**

- 👁 reaction is visible on the second message during the busy window.
- After the first turn finishes, the second message produces its own
  agent turn (in addition to the streamed first one).
- **No** "Queued for the next turn." chat reply is sent for the second
  message — free-text enqueue is silent per D-13.
- `/tmp/uat-36.17.1-gateway.log` shows a `SessionQueue: enqueued event
  while agent busy` debug line for the second message.

---

## Scenario 2: `/queue` command path

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
- `/tmp/uat-36.17.1-gateway.log` shows a `SessionQueue: capacity reached,
  message dropped` warn line tagged with the session key.

4. **Pitfall 6 verification — no re-delivery:**

   Wait ~30 seconds. In your Telegram client, observe whether the 129th
   message receives ANY additional reactions or processing. Also check
   `/tmp/uat-36.17.1-gateway.log` for any duplicate `msg_id 129`
   processing:

   ```bash
   grep "msg 129\|msg_129\|<id_you_used_for_129>" /tmp/uat-36.17.1-gateway.log
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
- `/tmp/uat-36.17.1-gateway.log` shows session removal followed by no
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
     /tmp/uat-36.17.1-gateway.log | tail -20
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

- [ ] **Scenario 1** — Busy-enqueue silent: 👁 reaction visible, second
      message replays after first turn, no extra chat reply.
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

Verifier (your name): ____________________

Date (YYYY-MM-DD): ____________________

Notes / failures (attach screenshots + log excerpts if any scenario fails):

```
[paste here]
```

---

*Phase: 36.17.1 — in-mem-fifo-queuing-parity-of-python-deque-for-chat-sessions*
*Runbook authored: 2026-05-27*
