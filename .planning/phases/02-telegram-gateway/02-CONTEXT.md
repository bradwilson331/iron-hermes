# Phase 2: Telegram Gateway - Context

**Gathered:** 2026-04-01
**Status:** Ready for planning

<domain>
## Phase Boundary

Wire Telegram long polling to the agent loop — a working Telegram bot that receives messages (text, images, documents, PDFs), runs them through the agent loop with tool use, streams responses back with progressive message editing, and handles multiple concurrent users reliably. Includes user whitelist, slash commands, session management, and error recovery.

</domain>

<decisions>
## Implementation Decisions

### Streaming UX
- **D-01:** Block cursor `█` appended to end of text while LLM is generating
- **D-02:** Show tool name during execution — append "⚙️ Running: {tool_name}..." as a temporary status line in the message
- **D-03:** Plain text (no parse_mode) during streaming edits; switch to Markdown parse mode on final edit only — avoids broken formatting mid-stream
- **D-04:** Chain messages at natural breakpoints (paragraphs/sentences) when response exceeds 4096-char Telegram limit — preserves full response

### Message scope
- **D-05:** Process text messages, documents, PDFs, and images — not just text
- **D-06:** Images: download from Telegram and pass to LLM as vision/image input (requires multimodal support in LLM client)
- **D-07:** Documents/PDFs: download from Telegram, extract text content, inject as user message context
- **D-08:** Maximum file size: 20MB (Telegram Bot API maximum)
- **D-09:** Group chats: respond only when @mentioned — no response to unmentioned messages
- **D-10:** User whitelist configured as Telegram user IDs (numeric) in config YAML (`~/.ironhermes/config.yaml`)
- **D-11:** Whitelist applies everywhere — both DMs and group @mentions. Unauthorized users are silently ignored.
- **D-12:** Empty whitelist = deny all — secure by default, forces explicit configuration

### Session lifecycle
- **D-13:** Slash commands: `/start` (SOUL.md-driven greeting), `/new` (fresh session, archives old), `/clear` (wipe history), `/help` (show commands)
- **D-14:** Session timeout: 24 hours of inactivity, configurable in config YAML
- **D-15:** Bot greeting on `/start`: make an LLM call with the loaded SOUL.md personality to generate an in-character introduction
- **D-16:** Continuous typing indicator — send `sendChatAction("typing")` every 5 seconds throughout the entire agent run, including tool execution
- **D-17:** Auto-register commands with Telegram via `setMyCommands` API on bot startup — no manual BotFather setup needed

### Error presentation
- **D-18:** Agent errors mid-response: append "⚠️ Something went wrong, please try again" to whatever was already streamed — preserves partial output
- **D-19:** Telegram rate limits (429): silently retry with backoff up to 3 times, then tell user "Bot is being rate limited, please wait"
- **D-20:** Tool execution errors: fed back to the LLM as context — agent decides what to tell the user (matches existing CLI agent loop behavior)

### Concurrency UX
- **D-21:** Overlapping messages from same user: queue new message until current agent run completes, then process — preserves conversation order
- **D-22:** Acknowledge queued messages with 👀 emoji reaction via existing `add_reaction` API

### Claude's Discretion
- Exact message splitting algorithm for chain messages (paragraph vs sentence boundaries)
- PDF text extraction library choice
- Queue depth limit per user (if any)
- Exact /help text content
- sendChatAction timing details (5s is guidance, can adjust)

</decisions>

<specifics>
## Specific Ideas

- Bot should feel "alive" on first contact — SOUL.md-driven greeting, not a static welcome message
- Silent ignore for unauthorized users — bot appears offline to them, doesn't reveal its existence
- Queue + emoji reaction pattern for overlapping messages — user knows message was received without cluttering chat
- Tool execution visibility matches hermes-agent pattern — agent mediates all communication, tools don't talk directly to users

</specifics>

<canonical_refs>
## Canonical References

**Downstream agents MUST read these before planning or implementing.**

### Telegram gateway architecture
- `.planning/REQUIREMENTS.md` §Telegram Gateway — TG-01 through TG-08 requirements
- `.planning/REQUIREMENTS.md` §Async Infrastructure — ASYNC-01 through ASYNC-03 requirements
- `.planning/ROADMAP.md` §Phase 2 — Key technical decisions (CancellationToken, channel-based dispatch, StreamConsumer, backoff parameters)

### Existing gateway code
- `crates/ironhermes-gateway/src/telegram.rs` — TelegramAdapter with Bot API types, long polling skeleton, send/edit/delete/reactions
- `crates/ironhermes-gateway/src/adapter.rs` — PlatformAdapter and MessageHandler traits
- `crates/ironhermes-gateway/src/runner.rs` — GatewayRunner skeleton (needs handler wiring, Arc wrapping)
- `crates/ironhermes-gateway/src/session.rs` — SessionStore and GatewaySession (needs Arc<RwLock> wrapping, timeout support)

### Agent loop integration
- `crates/ironhermes-agent/src/agent_loop.rs` — AgentLoop with streaming support (StreamCallback), tool progress callback, context compression

### Configuration
- `crates/ironhermes-core/` — Config struct, MessageEvent, ChatMessage types

</canonical_refs>

<code_context>
## Existing Code Insights

### Reusable Assets
- `TelegramAdapter`: Full Bot API client (sendMessage, editMessageText, deleteMessage, setMessageReaction, getUpdates) — needs streaming bridge and CancellationToken but API calls are ready
- `SessionStore` + `GatewaySession`: In-memory session management with chat_id keying — needs Arc<RwLock> wrapping and timeout/expiry logic
- `AgentLoop::with_streaming(callback)`: Existing stream callback pattern — needs bridging to Telegram edit_message calls
- `AgentLoop::with_tool_progress(callback)`: Tool progress callback — can power the "⚙️ Running: tool_name" status display
- `tg_message_to_event()`: Message conversion already handles DM/group/channel chat types

### Established Patterns
- `Arc<ToolRegistry>` already used in AgentLoop — same pattern extends to sharing across concurrent gateway tasks
- Streaming via `mpsc::Receiver<StreamEvent>` in LlmClient — channel-based pattern can be reused for polling-to-processing bridge
- Config loaded from YAML at `~/.ironhermes/` — whitelist and session timeout config fits here

### Integration Points
- `MessageHandler` trait needs rework: currently returns `Result<String>`, needs to support streaming bridge for progressive edits
- `GatewayRunner::start()` doesn't wire the handler to adapters — the actual adapter.start(handler) call is missing
- CLI main.rs creates AgentLoop with config — gateway needs similar setup path (separate binary or subcommand)
- `PlatformAdapter::start()` takes `Box<dyn MessageHandler>` — needs Arc for sharing across concurrent handlers

</code_context>

<deferred>
## Deferred Ideas

None — discussion stayed within phase scope

</deferred>

---

*Phase: 02-telegram-gateway*
*Context gathered: 2026-04-01*
