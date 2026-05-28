---
slug: research-output-not-delivered
status: resolved
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

**hypothesis:** CONFIRMED — two independent bugs, both fixed.
**next_action:** None — resolved.

## Evidence

- timestamp: 2026-05-28T15:30:00Z
  file: crates/ironhermes-gateway/src/stream_consumer.rs
  lines: 86-91
  finding: |
    BUG 1 (PRIMARY — root cause of missing delivery): The `flush(final_edit=true)` branch
    directly calls `edit_message_markdown` with the full buffer content, with NO length check
    and NO overflow chunking. The overflow chunker (lines 100-125) lives exclusively in the
    `final_edit=false` branch.

    When the agent produces a response ≥4096 chars, `flush(true)` sends the entire content
    to Telegram's `editMessageText` API. Telegram rejects this with:
      {"ok":false,"error_code":400,"description":"Bad Request: message is too long"}

    The call site in handler.rs (lines 1162 and 1177) uses `let _ = consumer.flush(true).await`
    — the error is silently discarded. The placeholder █ message never gets replaced, and
    nothing arrives in Telegram. No log entry is emitted.

    The streaming-partial path (flush(false)) correctly handles overflow by chunking at
    paragraph boundaries during generation. The final-response path (flush(true)) was
    missing this entirely — a code path divergence.

- timestamp: 2026-05-28T15:35:00Z
  file: crates/ironhermes-tools/src/delegate_task.rs
  lines: 726, 732-734
  finding: |
    BUG 2 (SECONDARY — explains the delegate_task failure at 14:49:26Z): The JSON schema
    exposed to the LLM uses `"required": []` (empty). Neither `task` nor `tasks` appears as
    required at the schema level. The enforcement at line 732-734 is runtime-only:
      if args.get("tasks").is_none() && args.get("task").is_none() {
          anyhow::bail!("Either 'task' (single mode) or 'tasks' (batch mode) is required");
      }

    When the LLM calls `delegate_task` without providing `task` or `tasks` (e.g. with only
    metadata fields like `toolsets`), the schema does not prevent it — only the runtime check
    does. This produces the tool error seen in the UAT log at 14:49:26Z:
      "Tool 'delegate_task' failed: Either 'task' (single mode) or 'tasks' (batch mode) is required"

    The LLM then consumes turns recovering. Even after recovery, Bug 1 suppresses delivery.

## Eliminated

- **Phase 36.17.2 regression** — ELIMINATED via the live UAT signoff (this issue exists independently of the queue refactor; the same outbound delivery path was used before 36.17.2 and would have failed identically).
- **Telegram emoji reaction issue (Plan 06)** — ELIMINATED. The 👀 fix landed and zero `REACTION_INVALID` lines appear in the UAT log. Reactions are unrelated to message-body delivery.
- **Chunker error-swallowing during streaming** — ELIMINATED. The non-final flush path correctly propagates errors from `send_message` via `?`. The bug is in the final-flush path only.
- **Sub-agent results not surfacing** — ELIMINATED as the primary cause. The sub-agent output does reach the parent's final response text. Bug 1 then silently drops that text at delivery time.
- **Streaming state machine corruption** — ELIMINATED. The channel close + `flush(true)` sequence is correct; the bug is inside `flush(true)` itself.

## Resolution

**Root cause:**
`StreamConsumer::flush(true)` (the final-edit path) called `edit_message_markdown` directly on the full buffer without checking length. Telegram rejects `editMessageText` with content > 4096 chars (HTTP 400). The error was silently discarded at the call site (`let _ = consumer.flush(true).await`), so the placeholder message was never replaced and nothing arrived in Telegram.

**Fix applied:**

1. `crates/ironhermes-gateway/src/stream_consumer.rs` — The `final_edit=true` branch now
   chunks the buffer using the same `find_split_point` logic as the non-final path. The first
   chunk edits the placeholder with plain text; each subsequent chunk is sent as a new
   `send_message` call; the final chunk is edited with `edit_message_markdown`. A new
   regression test `test_final_flush_overflow_chunks_long_content` covers the ≥4096-char
   final-flush case.

2. `crates/ironhermes-tools/src/delegate_task.rs` — The JSON schema now uses:
   ```json
   "oneOf": [
     { "required": ["task"] },
     { "required": ["tasks"] }
   ]
   ```
   instead of `"required": []`. The LLM sees the constraint at schema parse time, not only
   at runtime. The corresponding test was updated to verify the `oneOf` structure.

**Verification:**
- `cargo check -p ironhermes-gateway -p ironhermes-tools` — clean (pre-existing warnings only)
- `cargo test -p ironhermes-gateway --lib -- stream_consumer` — 13/13 passed
- `cargo test -p ironhermes-tools --lib -- delegate_task` — 50/50 passed

**Post-fix UAT instruction:**
Send a prompt that generates ≥4096 chars output (e.g. "Write 5000 words on the history of FIFO queues with citations."). Confirm that multiple Telegram messages arrive in order, the first replacing the █ placeholder, subsequent chunks appearing as new messages.
