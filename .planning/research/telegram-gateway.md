# Research: Wiring Telegram Polling to the Agent Loop in IronHermes

**Researched:** 2026-04-01
**Confidence:** MEDIUM (training data only -- web search/fetch unavailable for live verification)

---

## 1. Rust Telegram Bot Crates

### Option A: teloxide (RECOMMENDED AGAINST for this project)

teloxide is the most popular Rust Telegram framework (~8M+ downloads on crates.io as of early 2025). It provides a high-level, opinionated framework with dispatcher pattern, dialogue state machines, and built-in command parsing.

**Pros:**
- Most mature Rust Telegram crate, large community
- Built-in long polling with error recovery
- Typed Bot API bindings (auto-generated from Telegram schema)
- Dialogue/FSM support for stateful conversations

**Cons:**
- Heavy framework -- pulls in its own dispatcher, handler chain, dependency injection
- Opinionated architecture conflicts with IronHermes' existing `PlatformAdapter` trait
- You would fight the framework: teloxide wants to own the message routing, but IronHermes already has `MessageHandler`, `SessionStore`, and the `AgentLoop`
- Adds ~15+ transitive dependencies
- The dispatcher pattern is designed for command bots, not agent loops where every message goes through the same pipeline

### Option B: frankenstein

frankenstein is a lower-level Telegram Bot API client. It provides typed request/response structs for every API method but no dispatcher or framework.

**Pros:**
- Clean typed API bindings without framework overhead
- You call methods directly: `api.send_message(&params).await`
- Async support via reqwest
- Maintained, tracks Telegram Bot API updates

**Cons:**
- Still adds a dependency for something IronHermes already does
- Its type system is different from IronHermes' existing `TgMessage`, `TgUpdate`, etc.
- Migration cost: you'd need to map between frankenstein types and your `MessageEvent`

### Option C: Keep the hand-rolled client (RECOMMENDED)

IronHermes already has a working `TelegramAdapter` with:
- `api_call<T>()` generic method for any Bot API endpoint
- Typed structs: `TgUpdate`, `TgMessage`, `TgUser`, `TgChat`, `TelegramResponse<T>`
- `send_message`, `edit_message`, `delete_message`, `add_reaction`
- Long polling loop in `start()` with offset tracking
- `tg_message_to_event()` conversion to the shared `MessageEvent` type

**Why keep it:**
1. **It already works.** The polling loop fetches updates, parses them, and converts to `MessageEvent`. The gap is not in the Telegram client -- it is in wiring the handler to the agent loop.
2. **Zero new dependencies.** You already use `reqwest` and `serde`. The Telegram Bot API is simple JSON-over-HTTP. A typed wrapper crate adds dependency weight for no functional gain.
3. **Full control over error recovery.** The existing code already has the error/retry structure. Adding exponential backoff and conflict detection (see Section 2) is straightforward.
4. **Type alignment.** Your types map directly to `MessageEvent` with no adapter layer. With teloxide or frankenstein, you would need a translation layer.

**What to add to the existing client:**
- `sendChatAction` (typing indicator) -- one more `api_call`
- Photo/document/voice handling (parse `TgUpdate.message.photo`, `.document`, `.voice`)
- `getFile` + download for media attachments
- `reply_to_message_id` parameter in `send_message`

These are each ~10 lines of code. Not worth a framework dependency.

---

## 2. Long Polling Patterns in Rust

### Current State

The existing `TelegramAdapter::start()` spawns a `tokio::spawn` task that does long polling correctly:
- 30-second timeout on `getUpdates`
- Offset tracking (`offset = Some(update.update_id + 1)`)
- Basic error recovery (5-second sleep on failure)

### What Needs Improvement

#### 2a. Exponential Backoff

The current code uses a flat 5-second retry. This is fine for transient blips but bad for sustained outages (wastes resources, can trigger Telegram rate limits).

```rust
// Recommended: exponential backoff with jitter
struct BackoffState {
    consecutive_errors: u32,
    base_delay: Duration,
    max_delay: Duration,
}

impl BackoffState {
    fn new() -> Self {
        Self {
            consecutive_errors: 0,
            base_delay: Duration::from_secs(1),
            max_delay: Duration::from_secs(60),
        }
    }

    fn next_delay(&mut self) -> Duration {
        self.consecutive_errors += 1;
        let delay = self.base_delay * 2u32.pow(self.consecutive_errors.min(6));
        let delay = delay.min(self.max_delay);
        // Add jitter: +/- 25%
        let jitter_range = delay.as_millis() as f64 * 0.25;
        let jitter = rand::random::<f64>() * jitter_range * 2.0 - jitter_range;
        Duration::from_millis((delay.as_millis() as f64 + jitter) as u64)
    }

    fn reset(&mut self) {
        self.consecutive_errors = 0;
    }
}
```

**Key rule from hermes-agent reference:** The Python Telegram adapter uses `5s, 10s, 20s, 40s, 60s cap` for network errors. Match this.

#### 2b. Conflict Detection (409 errors)

Telegram returns HTTP 409 when two bot instances poll with the same token. The Python hermes-agent handles this as a fatal error after N retries. IronHermes should do the same:

```rust
Ok(resp) if !resp.ok => {
    if resp.error_code == Some(409) {
        conflict_count += 1;
        if conflict_count >= 5 {
            error!("Telegram polling conflict: another instance is running with this token");
            // Signal fatal error to the runner
            break;
        }
        warn!("Telegram 409 conflict ({}/5), retrying...", conflict_count);
        tokio::time::sleep(Duration::from_secs(2)).await;
    } else {
        // Other API errors: use backoff
        let delay = backoff.next_delay();
        warn!("Telegram API error {}: {}, retrying in {:?}",
            resp.error_code.unwrap_or(0),
            resp.description.unwrap_or_default(),
            delay
        );
        tokio::time::sleep(delay).await;
    }
}
```

#### 2c. Graceful Shutdown

The current code uses `AtomicBool` + `handle.abort()`. This is almost right, but `abort()` is violent -- it cancels the task immediately without cleanup. Better pattern:

```rust
use tokio_util::sync::CancellationToken;

pub struct TelegramAdapter {
    token: String,
    http: Client,
    cancel: CancellationToken,
    poll_handle: Option<tokio::task::JoinHandle<()>>,
}

// In the poll loop:
loop {
    tokio::select! {
        _ = cancel.cancelled() => {
            info!("Telegram polling shutting down gracefully");
            break;
        }
        result = http.post(&url).json(&params).send() => {
            // ... handle result ...
        }
    }
}

// In stop():
async fn stop(&mut self) -> Result<()> {
    self.cancel.cancel();
    if let Some(handle) = self.poll_handle.take() {
        // Wait up to 5 seconds for graceful completion
        match tokio::time::timeout(Duration::from_secs(5), handle).await {
            Ok(_) => info!("Telegram adapter stopped cleanly"),
            Err(_) => {
                warn!("Telegram adapter did not stop in time, aborting");
                // handle is dropped here, which aborts the task
            }
        }
    }
    Ok(())
}
```

Add `tokio-util` to workspace dependencies (you may already have it transitively via tokio-stream).

#### 2d. Network Reconnection

The Python adapter has a dedicated `_handle_polling_network_error` method that detects when the host loses connectivity (Mac sleep, WiFi switch, VPN reconnect). The long-poll connection silently dies and the bot never recovers without this.

In Rust, `reqwest` will surface this as a connection error. The backoff handles it, but add explicit logging:

```rust
Err(e) if e.is_connect() || e.is_timeout() => {
    warn!("Network connectivity issue: {}, backing off...", e);
    let delay = backoff.next_delay();
    tokio::time::sleep(delay).await;
}
```

---

## 3. Wiring Polling to the Agent Loop

This is the core architectural question. The current code has a gap: `GatewayRunner::start()` creates the `TelegramAdapter` but never calls `adapter.start(handler)` -- it just waits for ctrl+c.

### 3a. The Current Problem

```
TelegramAdapter::start() takes Box<dyn MessageHandler>
  -> MessageHandler::handle() returns Result<String>
  -> The poll loop spawns a task per message, calls handler.handle()
  -> Sends the response back inline

But:
  - AgentLoop::run() takes Vec<ChatMessage> and returns AgentResult
  - There's no bridge between MessageHandler and AgentLoop
  - Session management (conversation history) is not wired
  - Streaming is not connected
```

### 3b. Recommended Architecture: Channel-Based Bridge

Follow the Python hermes-agent pattern, adapted for Rust's ownership model:

```
Telegram Poll Loop
    |
    v
MessageEvent  -->  mpsc channel  -->  Message Dispatcher Task
                                           |
                                           v
                                    Per-Session Processing
                                    (spawn_blocking or tokio::spawn)
                                           |
                                           v
                                    SessionStore.get_or_create()
                                    Build messages Vec<ChatMessage>
                                    AgentLoop::run(messages)
                                           |
                                           v
                                    Stream Consumer (edits message)
                                    OR
                                    Final response -> send_message()
```

#### Step 1: Replace the inline handler with a channel

```rust
use tokio::sync::mpsc;

pub struct TelegramGateway {
    adapter: TelegramAdapter,
    sessions: Arc<RwLock<SessionStore>>,
    agent_factory: Arc<dyn AgentFactory>,
    event_tx: mpsc::Sender<MessageEvent>,
    event_rx: Option<mpsc::Receiver<MessageEvent>>,
}
```

The poll loop sends `MessageEvent` into the channel instead of calling the handler directly. A separate dispatcher task reads from the channel.

#### Step 2: The Dispatcher Task

```rust
async fn dispatch_loop(
    mut rx: mpsc::Receiver<MessageEvent>,
    adapter: Arc<TelegramAdapter>,
    sessions: Arc<RwLock<SessionStore>>,
    agent_factory: Arc<dyn AgentFactory>,
    cancel: CancellationToken,
) {
    // Track active sessions to support interruption
    let active: Arc<DashMap<String, CancellationToken>> = Arc::new(DashMap::new());

    loop {
        tokio::select! {
            _ = cancel.cancelled() => break,
            Some(event) = rx.recv() => {
                let session_key = format!("{}:{}", event.chat_id, event.sender_id);

                // If session is already processing, signal interrupt
                if let Some(existing_cancel) = active.get(&session_key) {
                    existing_cancel.cancel();
                    // The existing task will finish, then the new message
                    // gets picked up on the next iteration
                }

                let adapter = adapter.clone();
                let sessions = sessions.clone();
                let agent_factory = agent_factory.clone();
                let active = active.clone();
                let session_cancel = CancellationToken::new();
                active.insert(session_key.clone(), session_cancel.clone());

                tokio::spawn(async move {
                    process_message(event, adapter, sessions, agent_factory, session_cancel).await;
                    active.remove(&session_key);
                });
            }
        }
    }
}
```

#### Step 3: The per-message processor

```rust
async fn process_message(
    event: MessageEvent,
    adapter: Arc<TelegramAdapter>,
    sessions: Arc<RwLock<SessionStore>>,
    agent_factory: Arc<dyn AgentFactory>,
    cancel: CancellationToken,
) {
    // 1. Send typing indicator
    let _ = adapter.send_chat_action(&event.chat_id, "typing").await;

    // 2. Get or create session, add user message
    let messages = {
        let mut store = sessions.write().await;
        let key = SessionKey::new(Platform::Telegram, &event.chat_id)
            .with_user(&event.sender_id);
        let session = store.get_or_create(key, "default-model");
        session.add_message(ChatMessage::user(&event.content));
        session.messages.clone()
    };

    // 3. Create agent and run
    let agent = agent_factory.create();

    // 4. Run with streaming (see Section 4)
    let (stream_tx, stream_rx) = tokio::sync::mpsc::channel::<String>(64);
    let stream_consumer = StreamConsumer::new(
        adapter.clone(),
        event.chat_id.clone(),
        stream_rx,
    );
    let consumer_handle = tokio::spawn(stream_consumer.run());

    let stream_callback: StreamCallback = Box::new(move |delta: &str| {
        let _ = stream_tx.try_send(delta.to_string());
    });

    let agent = agent.with_streaming(stream_callback);
    let result = agent.run(messages).await;

    // Signal stream complete
    drop(stream_tx); // Dropping the sender signals completion
    let _ = consumer_handle.await;

    // 5. Store assistant response in session
    match result {
        Ok(agent_result) => {
            if let Some(ref response) = agent_result.final_response {
                let mut store = sessions.write().await;
                let key = SessionKey::new(Platform::Telegram, &event.chat_id)
                    .with_user(&event.sender_id);
                if let Some(session) = store.get_mut(&key) {
                    session.add_message(ChatMessage::assistant(response));
                }
            }
        }
        Err(e) => {
            error!("Agent error: {}", e);
            let _ = adapter.send_message(&event.chat_id, "Sorry, something went wrong.", None).await;
        }
    }
}
```

#### Step 4: The AgentFactory trait

The agent loop needs to be created per-request (it holds mutable state like message history). Use a factory:

```rust
#[async_trait]
pub trait AgentFactory: Send + Sync {
    fn create(&self, model: &str) -> AgentLoop;
}

pub struct DefaultAgentFactory {
    client_config: LlmClientConfig,
    tool_registry: Arc<ToolRegistry>,
    max_iterations: usize,
}

impl AgentFactory for DefaultAgentFactory {
    fn create(&self, model: &str) -> AgentLoop {
        let client = LlmClient::new(self.client_config.clone(), model);
        AgentLoop::new(client, self.tool_registry.clone(), self.max_iterations)
    }
}
```

### 3c. Why NOT actors (e.g., actix, ractor)

Actor frameworks add complexity without benefit here. The pattern is simple:
- One poll task produces events
- One dispatcher task routes them
- Per-session tasks process them

`tokio::spawn` + `mpsc` channels are the right abstraction level. This matches what the Python hermes-agent does with `asyncio.create_task`.

---

## 4. Streaming Responses to Telegram

### How the Python hermes-agent does it (GatewayStreamConsumer)

The Python `GatewayStreamConsumer` is the gold standard reference. Key design:

1. **Send initial message** on first token batch
2. **Edit the message** as more tokens arrive
3. **Rate limit edits** to every 300ms or 40 characters (whichever comes first)
4. **Show cursor** (`" ▉"`) during streaming, remove on completion
5. **Handle message overflow** -- if response exceeds 4096 chars, finalize current message and start a new one
6. **Thread-safe bridge** -- agent runs in a thread pool, consumer runs in async context

### Rust Implementation

```rust
pub struct StreamConsumer {
    adapter: Arc<TelegramAdapter>,
    chat_id: String,
    rx: mpsc::Receiver<String>,
    edit_interval: Duration,
    buffer_threshold: usize,
    cursor: String,
}

impl StreamConsumer {
    pub fn new(
        adapter: Arc<TelegramAdapter>,
        chat_id: String,
        rx: mpsc::Receiver<String>,
    ) -> Self {
        Self {
            adapter,
            chat_id,
            rx,
            edit_interval: Duration::from_millis(300),
            buffer_threshold: 40,
            cursor: " \u{2589}".to_string(), // " ▉"
        }
    }

    pub async fn run(mut self) -> Option<String> {
        let mut accumulated = String::new();
        let mut message_id: Option<String> = None;
        let mut last_edit = Instant::now();
        let mut last_sent_text = String::new();
        let safe_limit: usize = 4096 - self.cursor.len() - 100;

        loop {
            // Drain available tokens with a timeout
            match tokio::time::timeout(Duration::from_millis(50), self.rx.recv()).await {
                Ok(Some(delta)) => {
                    accumulated.push_str(&delta);
                }
                Ok(None) => {
                    // Channel closed = stream complete
                    // Final edit without cursor
                    if let Some(ref mid) = message_id {
                        if accumulated != last_sent_text && !accumulated.is_empty() {
                            let _ = self.adapter.edit_message(&self.chat_id, mid, &accumulated).await;
                        }
                    } else if !accumulated.is_empty() {
                        let _ = self.adapter.send_message(&self.chat_id, &accumulated, None).await;
                    }
                    return Some(accumulated);
                }
                Err(_) => {
                    // Timeout -- check if we should flush
                }
            }

            // Decide whether to flush
            let elapsed = last_edit.elapsed();
            let should_flush = elapsed >= self.edit_interval
                || accumulated.len() >= self.buffer_threshold;

            if should_flush && !accumulated.is_empty() {
                // Handle overflow: split if too long
                while accumulated.len() > safe_limit && message_id.is_some() {
                    let split_at = accumulated[..safe_limit]
                        .rfind('\n')
                        .filter(|&pos| pos >= safe_limit / 2)
                        .unwrap_or(safe_limit);
                    let chunk = &accumulated[..split_at];
                    if let Some(ref mid) = message_id {
                        let _ = self.adapter.edit_message(&self.chat_id, mid, chunk).await;
                    }
                    accumulated = accumulated[split_at..].trim_start_matches('\n').to_string();
                    message_id = None;
                    last_sent_text.clear();
                }

                let display_text = format!("{}{}", accumulated, self.cursor);

                if display_text == last_sent_text {
                    continue;
                }

                if let Some(ref mid) = message_id {
                    let _ = self.adapter.edit_message(&self.chat_id, mid, &display_text).await;
                } else {
                    match self.adapter.send_message(&self.chat_id, &display_text, None).await {
                        Ok(resp) => message_id = Some(resp.message_id),
                        Err(e) => error!("Failed to send streaming message: {}", e),
                    }
                }
                last_sent_text = display_text;
                last_edit = Instant::now();
            }
        }
    }
}
```

### Telegram Rate Limits

These are not officially published in full, but well-established from community experience:

| Limit | Value | Notes |
|-------|-------|-------|
| Messages per second (same chat) | ~1/s | Soft limit, 429 after burst |
| Messages per second (global) | 30/s | Across all chats |
| Group messages | 20/min per group | Stricter for groups |
| `editMessageText` | Same as send | Counts toward the rate limit |
| Message length | 4096 chars | UTF-8 characters, not bytes |
| Inline keyboard + text | 4096 chars | Text portion only |

**Practical implication:** The 300ms edit interval from the Python implementation is close to the floor. Do not edit faster than ~3/s to a single chat. The Python code's approach of buffering + rate-limiting is correct.

**429 handling:** When you get HTTP 429, the response includes a `retry_after` field (seconds). Respect it:

```rust
if resp.error_code == Some(429) {
    if let Some(params) = resp.parameters {
        let retry_after = params.retry_after.unwrap_or(5);
        warn!("Rate limited, waiting {}s", retry_after);
        tokio::time::sleep(Duration::from_secs(retry_after as u64)).await;
    }
}
```

Add to your `TelegramResponse` struct:

```rust
#[derive(Debug, Deserialize)]
struct TelegramResponse<T> {
    ok: bool,
    result: Option<T>,
    error_code: Option<i32>,
    description: Option<String>,
    parameters: Option<TgResponseParameters>,
}

#[derive(Debug, Deserialize)]
struct TgResponseParameters {
    retry_after: Option<i32>,
    migrate_to_chat_id: Option<i64>,
}
```

---

## 5. Session Management

### Current State

IronHermes has a `SessionStore` with `SessionKey` (platform + chat_id + optional user_id) and `GatewaySession` (messages vec, timestamps, model). This is solid groundwork.

### What to Add

#### 5a. Session Key Strategy

Follow the Python hermes-agent pattern:
- **DMs:** Key by `telegram:{chat_id}` (chat_id == user_id in DMs)
- **Groups:** Key by `telegram:{chat_id}:{user_id}` (each user gets their own conversation context within the group)
- **Configurable:** The Python code has `group_sessions_per_user` flag. Start with per-user sessions in groups.

The existing `SessionKey::with_user()` already supports this. Use it for groups:

```rust
let key = match event.chat_type.as_str() {
    "dm" => SessionKey::new(Platform::Telegram, &event.chat_id),
    _ => SessionKey::new(Platform::Telegram, &event.chat_id)
            .with_user(&event.sender_id),
};
```

#### 5b. Session Reset Policy

When to start a fresh conversation:

| Trigger | Behavior |
|---------|----------|
| `/reset` or `/new` command | Clear messages, keep session |
| Idle timeout (configurable, default 1 hour) | Clear messages on next message |
| Max message count (e.g., 100 turns) | Auto-compress or reset |
| Context window pressure | Trigger `ContextCompressor` |

```rust
impl GatewaySession {
    pub fn should_reset(&self, idle_timeout: Duration) -> bool {
        let idle = Utc::now() - self.updated_at;
        idle > chrono::Duration::from_std(idle_timeout).unwrap_or(chrono::Duration::hours(1))
    }
}
```

#### 5c. System Prompt Injection

The Python hermes-agent injects gateway context into the system prompt so the agent knows where it is:

```
You are chatting on Telegram.
Platform: Telegram
Chat: {group_name or "DM"}
User: {sender_name}
```

Do the same in IronHermes when building the messages for the agent:

```rust
fn build_agent_messages(event: &MessageEvent, session: &GatewaySession, base_system_prompt: &str) -> Vec<ChatMessage> {
    let context = format!(
        "{}\n\n---\nPlatform: Telegram\nChat type: {}\nChat: {}\nUser: {}",
        base_system_prompt,
        event.chat_type,
        event.chat_name.as_deref().unwrap_or("DM"),
        event.sender_name.as_deref().unwrap_or("Unknown"),
    );

    let mut messages = vec![ChatMessage::system(context)];
    messages.extend(session.messages.clone());
    messages
}
```

#### 5d. Persistence

The current `SessionStore` is in-memory (HashMap). For a first pass, this is fine. For production:

1. **Phase 1:** In-memory (current). Sessions lost on restart. Acceptable for MVP.
2. **Phase 2:** SQLite persistence using the existing `rusqlite` dependency. Serialize `Vec<ChatMessage>` as JSON into a sessions table. Load on startup, write-through on each message.

```sql
CREATE TABLE IF NOT EXISTS gateway_sessions (
    key TEXT PRIMARY KEY,
    session_id TEXT NOT NULL,
    messages TEXT NOT NULL,  -- JSON array
    model TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
```

#### 5e. Concurrency Safety

The current `SessionStore` uses `HashMap` without synchronization. Wrap it:

```rust
// In the gateway
sessions: Arc<tokio::sync::RwLock<SessionStore>>
```

Or use `DashMap` for lock-free concurrent access (add `dashmap` crate). For this use case, `RwLock` is simpler and sufficient -- contention is low (one write per message, reads are cheap).

---

## 6. Putting It All Together: Implementation Plan

### Phase 1: Wire the basic loop (no streaming)

1. Fix `GatewayRunner::start()` to actually start adapters with a handler
2. Create `GatewayMessageHandler` implementing `MessageHandler` that bridges to `AgentLoop`
3. Wrap `SessionStore` in `Arc<RwLock<>>`
4. Add `sendChatAction` typing indicator
5. Add exponential backoff to poll loop
6. Add graceful shutdown with `CancellationToken`

**Result:** Messages come in, go through the agent loop, responses go back. No streaming.

### Phase 2: Add streaming

1. Build `StreamConsumer` (port from Python `GatewayStreamConsumer`)
2. Wire `AgentLoop::with_streaming()` callback to `StreamConsumer`
3. Add rate limiting (300ms edit interval)
4. Handle message overflow (>4096 chars)
5. Add `TgResponseParameters` for 429 handling

**Result:** Responses stream in real-time with cursor, editing the message progressively.

### Phase 3: Robustness

1. Session reset on idle timeout
2. 409 conflict detection
3. Network reconnection handling
4. Interrupt support (new message cancels in-progress agent run)
5. Group message handling (per-user sessions)
6. `/reset`, `/new` commands

### Phase 4: Persistence and polish

1. SQLite session persistence
2. Media handling (photos, documents, voice)
3. Markdown formatting (Telegram MarkdownV2 is notoriously tricky -- see the Python `_escape_mdv2` function)
4. PII redaction for logs (hash chat_id, user_id)

---

## 7. Key Architectural Decisions

### Decision 1: Keep hand-rolled Telegram client

**Rationale:** The existing `TelegramAdapter` is 90% complete. The missing pieces are small (typing indicator, backoff, conflict detection). Adding teloxide would mean rewriting the adapter to fit teloxide's dispatcher, losing alignment with the `PlatformAdapter` trait.

### Decision 2: Channel-based message dispatch (not inline handler)

**Rationale:** The current design calls `handler.handle()` inline in the poll loop task. This blocks the loop during agent execution (which can take 30+ seconds with tool calls). A channel decouples polling from processing, matching the Python hermes-agent's `asyncio.create_task` pattern.

**However:** Looking at the current code more carefully, it already spawns `tokio::spawn` per message (line 123 of telegram.rs). So the poll loop is NOT blocked. The channel pattern is still better for interruption support (the dispatcher can track active sessions and cancel them), but the current spawn-per-message approach works for Phase 1.

**Revised recommendation:** For Phase 1, keep the spawn-per-message pattern but fix the handler wiring. Refactor to channels in Phase 3 when adding interrupt support.

### Decision 3: `Arc<TelegramAdapter>` for shared access

The current `PlatformAdapter` trait takes `&self` for send/edit/delete, which is correct. But the adapter needs to be shared between the poll loop (which owns it) and spawned message handler tasks (which need to send responses). Wrap in `Arc`:

```rust
// The adapter field in PlatformAdapter::start should be Arc<Self>
// Or better: split the adapter into a Poller (owns the loop) and a Sender (shared, Arc'd)
```

The cleanest approach: split `TelegramAdapter` into `TelegramPoller` (runs the loop) and `TelegramSender` (cloneable, used for send/edit/delete). The sender is just `(Client, String)` -- the HTTP client and token.

### Decision 4: StreamCallback is `Box<dyn Fn(&str) + Send + Sync>`

This already exists in `AgentLoop`. It is synchronous (called from within the async agent loop but not itself async). The stream consumer must bridge this to async edits. Use `mpsc::Sender::try_send()` (non-blocking) in the callback, and an async consumer task that drains the receiver. This matches the Python pattern exactly (`queue.Queue` bridging sync callback to async consumer).

---

## 8. Gaps and Open Questions

| Question | Impact | When to Resolve |
|----------|--------|-----------------|
| How does `AgentLoop` handle cancellation? | Needed for interrupt support | Phase 3 |
| Should sessions persist across restarts? | UX decision | Phase 4 |
| MarkdownV2 formatting | Telegram rejects malformed markdown | Phase 4 (use plain text initially) |
| Group @mention filtering | Bot should only respond when @mentioned in groups | Phase 3 |
| Webhook mode vs polling | Webhooks better for cloud deploy | Post-MVP |
| Multi-bot support | Multiple Telegram tokens | Post-MVP |

---

## 9. Reference: Python hermes-agent Architecture Summary

For future reference, here is how the Python codebase structures this:

```
gateway/run.py          -- GatewayRunner: starts all platform adapters
gateway/platforms/
    base.py             -- BasePlatformAdapter: handle_message() -> _process_message_background()
    telegram.py         -- TelegramPlatformAdapter: uses python-telegram-bot library
                           Handlers: _handle_text_message, _handle_command, _handle_media_message
                           Each calls self.handle_message(event) from base class
gateway/session.py      -- SessionSource, build_session_key, session reset policies
gateway/stream_consumer.py -- GatewayStreamConsumer: progressive message editing
                              on_delta() (sync) -> queue -> async run() -> send_or_edit()
```

Key pattern: The base adapter's `handle_message()` spawns a background task, tracks active sessions in `_active_sessions` dict, supports interruption via `asyncio.Event`, and queues pending messages for replay after the current task finishes. This is the pattern to port to Rust.
