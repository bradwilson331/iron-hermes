---
slug: research-output-not-delivered
status: investigating
trigger: agent completes a long-form research turn (≥4096 chars output) but the result never appears in the user's Telegram chat
created: 2026-05-28
updated: 2026-05-28
---

# Research Output Not Delivered to Telegram

## Symptoms

**Expected behavior:**
User sends "Research the history of FIFO queues from von Neumann onward and write 3000 words with citations." to the bot. Agent generates the research (multi-turn, may use web tools). Final assistant message is delivered to Telegram, chunked to fit Telegram's 4096-char per-message limit if necessary.

**Actual behavior:**
Agent loop runs to completion, consumes ~25 LLM calls (~$0.40 in API charges per OpenRouter billing), generates large output (one response was 19,087 tokens ≈ ~14k chars). User never receives any response in Telegram for that turn.

**Error messages:**
None observed in the gateway log. No `send_message` failures, no `with_rate_limit_retry` retries, no Telegram API errors. The agent completed silently and the response disappeared.

**Timeline:**
- Sent: 2026-05-28T14:49:11Z (`Received message from dispatch channel chat_id=7018949547 ... content=Research the history of FIFO queues...`)
- Agent loop: 14:49:12Z `Starting agent loop max_iterations=90`
- Sub-agent delegate (via `delegate_task` tool): 14:49:31Z `Starting agent loop max_iterations=50`
- LLM activity per OpenRouter dashboard: 14:49–14:55Z, ~25 calls, multiple multi-thousand-token outputs (largest single response: 19,087 output tokens)
- Agent completed: 14:52:59Z (`Agent completed, turns_used=3`) — likely the research's parent loop (3 turns)
- Telegram delivery: never

This was surfaced during the phase 36.17.2 live UAT (2026-05-28). Unrelated to phase 36.17.2's queue refactor — phase 36.17.2 only touches the inbound dispatch path. The outbound delivery layer (handler.rs → stream_consumer.rs → adapter.send_message) is what failed.

**Reproduction:**
1. Configure the gateway to point at an LLM provider that can generate long outputs (OpenRouter + Gemini 2.5 / 3.5 / Claude / GPT-4 all work).
2. Send a prompt that requires a long-form output (e.g. "write 3000 words on X with citations").
3. The agent will run for several minutes and produce a multi-thousand-token response.
4. Observe gateway log + Telegram client: agent loop completes, no `send_message` failure logged, but no message arrives in Telegram.

## Initial Suspicions

The outbound code path of interest:

- `crates/ironhermes-gateway/src/stream_consumer.rs:8` — `MAX_MESSAGE_LEN: usize = 4096`
- `stream_consumer.rs:101-114` — overflow handler: `if display.len() > MAX_MESSAGE_LEN { split_at_paragraph; send_message(remainder) }`
- `handler.rs:handle_with_multimodal` — orchestrates the agent loop and final reply delivery
- The agent loop's "final assistant text" path — what calls send_message with the agent's last output

Possible failure modes (to test in order):
1. **Chunker swallows error.** Send fails on chunk N, the loop continues without logging, none of the chunks land.
2. **Final assistant message bypasses the chunker.** The chunker is only used for streaming partials; the final response goes through a different path that hits Telegram's 4096-char limit and errors silently.
3. **Sub-agent results don't surface to the parent.** A `delegate_task` sub-agent generates the research; the parent agent's response is "I delegated, see results" but the sub-agent's output never makes it back into a Telegram send.
4. **The `delegate_task` failure at 14:49:26Z** (`Tool 'delegate_task' failed: Either 'task' (single mode) or 'tasks' (batch mode) is required`) puts the parent agent into a state where it consumes turns but never produces a final user-visible response.
5. **Streaming response state machine corruption.** The stream consumer's buffer gets into a state where it never flushes the final partial.

## Current Focus

**hypothesis:** The `delegate_task` tool failure at 14:49:26Z (missing `task`/`tasks` argument) put the parent agent into a degenerate state — it consumed turns retrying the delegate or pivoting, eventually exhausted `max_iterations` or produced a response that went to a side-channel (e.g. internal log, not Telegram). The user-visible result vanished because the actual research output lived inside the failed-then-retried delegate path's transcript, never reaching the final Telegram send.
**test:** Reproduce with a simpler long-form prompt that does NOT trigger `delegate_task` — e.g. "Write me a 4500-character story about queues with no external research." If THAT also fails to deliver, the bug is in the chunker / final-send path. If THAT delivers cleanly, the bug is in delegate_task semantics or the parent-loop's handling of sub-agent failure.
**expecting:** Either (a) chunker / send-path drops long messages silently (then we fix the chunker error handling), or (b) delegate_task failure shape causes the parent agent to lose its response (then we fix the delegate_task tool definition and/or the parent's error handling).
**next_action:** Read `handler.rs:handle_with_multimodal` end-to-end to map the final-response delivery path. Then read `stream_consumer.rs::push` and find_split_point. Then locate the `delegate_task` tool definition and its expected/actual schema.

## Evidence

(none yet — session starting)

## Eliminated

- **Phase 36.17.2 regression** — ELIMINATED via the live UAT signoff (this issue exists independently of the queue refactor; the same outbound delivery path was used before 36.17.2 and would have failed identically).
- **Telegram emoji reaction issue (Plan 06)** — ELIMINATED. The 👀 fix landed and zero `REACTION_INVALID` lines appear in the UAT log. Reactions are unrelated to message-body delivery.

## Resolution

(pending)
