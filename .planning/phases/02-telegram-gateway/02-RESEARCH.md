# Phase 02: Telegram Gateway - Research

**Researched:** 2026-04-01
**Domain:** Rust async concurrency, Telegram Bot API long polling, streaming message editing
**Confidence:** HIGH — architecture is fully derived from existing codebase inspection; no speculative choices

---

<user_constraints>
## User Constraints (from CONTEXT.md)

### Locked Decisions

**Streaming UX**
- D-01: Block cursor `█` appended to end of text while LLM is generating
- D-02: Show tool name during execution — append "⚙️ Running: {tool_name}..." as a temporary status line in the message
- D-03: Plain text (no parse_mode) during streaming edits; switch to Markdown parse mode on final edit only
- D-04: Chain messages at natural breakpoints (paragraphs/sentences) when response exceeds 4096-char Telegram limit

**Message scope**
- D-05: Process text messages, documents, PDFs, and images
- D-06: Images: download from Telegram and pass to LLM as vision/image input
- D-07: Documents/PDFs: download from Telegram, extract text content, inject as user message context
- D-08: Maximum file size: 20MB (Telegram Bot API maximum)
- D-09: Group chats: respond only when @mentioned
- D-10: User whitelist configured as Telegram user IDs (numeric) in config YAML (`~/.ironhermes/config.yaml`)
- D-11: Whitelist applies everywhere — both DMs and group @mentions. Unauthorized users silently ignored.
- D-12: Empty whitelist = deny all — secure by default

**Session lifecycle**
- D-13: Slash commands: `/start`, `/new`, `/clear`, `/help`
- D-14: Session timeout: 24 hours of inactivity, configurable in config YAML
- D-15: Bot greeting on `/start`: LLM call with SOUL.md personality to generate in-character introduction
- D-16: Continuous typing indicator — send `sendChatAction("typing")` every 5 seconds throughout agent run
- D-17: Auto-register commands via `setMyCommands` API on bot startup

**Error presentation**
- D-18: Agent errors mid-response: append "⚠️ Something went wrong, please try again"
- D-19: Telegram rate limits (429): silently retry with backoff up to 3 times, then inform user
- D-20: Tool execution errors: fed back to LLM as context

**Concurrency UX**
- D-21: Overlapping messages from same user: queue new message until current agent run completes
- D-22: Acknowledge queued messages with 👀 emoji reaction via `add_reaction` API

**Architecture (from ROADMAP.md)**
- Keep hand-rolled Telegram client (not teloxide/frankenstein)
- CancellationToken-based cooperative shutdown replacing AtomicBool + handle.abort()
- Arc<RwLock<SessionStore>> for safe session sharing across tokio tasks
- Arc<ToolRegistry> shared across concurrent agent runs
- Semaphore-bounded concurrency (default 4-8 concurrent agent runs)
- Supervisor pattern: JoinSet tracks active agent runs, drains on shutdown
- StreamConsumer with 300ms edit interval, cursor indicator, 4096-char overflow handling
- Exponential backoff with jitter (1s base, 60s cap) for polling failures
- 409 conflict detection (fatal after 5 retries)
- Channel-based message dispatch (mpsc) decoupling polling from processing

### Claude's Discretion
- Exact message splitting algorithm for chain messages (paragraph vs sentence boundaries)
- PDF text extraction library choice
- Queue depth limit per user (if any)
- Exact /help text content
- sendChatAction timing details (5s is guidance, can adjust)

### Deferred Ideas (OUT OF SCOPE)
None — discussion stayed within phase scope
</user_constraints>

---

<phase_requirements>
## Phase Requirements

| ID | Description | Research Support |
|----|-------------|------------------|
| TG-01 | Telegram long polling runs continuously, receives messages, dispatches to agent loop | Polling loop skeleton exists in TelegramAdapter; needs CancellationToken + mpsc channel dispatch wiring |
| TG-02 | Agent responses (including tool use results) sent back to originating Telegram chat | TelegramAdapter.send_message() exists; wiring to AgentResult.final_response needed |
| TG-03 | Streaming responses: progressive message editing as LLM chunks arrive | AgentLoop.with_streaming(callback) exists; StreamConsumer wraps it with 300ms throttled edit_message calls |
| TG-04 | Session management: chat_id maps to persistent conversation history via SessionStore | SessionStore + GatewaySession exist; need Arc<RwLock> wrapping and 24h timeout expiry |
| TG-05 | Graceful shutdown: CancellationToken-based cooperative shutdown of polling and in-flight runs | tokio-util 0.7.18 CancellationToken API; JoinSet drain pattern on shutdown |
| TG-06 | Concurrency limiting: Semaphore bounds maximum concurrent agent runs (default 4-8) | tokio::sync::Semaphore; acquire permit before spawning agent task |
| TG-07 | Error recovery: exponential backoff on polling failures, automatic reconnection | BackoffState struct with 1s base, 60s cap, jitter; 409 fatal after 5 retries |
| TG-08 | Typing indicator sent while agent is processing | Separate tokio task per agent run; sendChatAction("typing") every 5s, cancelled when run ends |
| ASYNC-01 | SessionStore wrapped in Arc<RwLock> for safe sharing across tokio tasks | SessionStore is a plain HashMap wrapper — wrap at GatewayRunner level |
| ASYNC-02 | ToolRegistry wrapped in Arc for sharing across concurrent agent runs | CLI already does Arc::new(registry); gateway replicates the same pattern |
| ASYNC-03 | Supervisor pattern for gateway subsystems with restart on transient failures | JoinSet::spawn for each agent run; polling loop restart via backoff loop |
</phase_requirements>

---

## Summary

Phase 2 is a wiring phase, not an invention phase. The Telegram Bot API client (`TelegramAdapter`) is ~90% complete — `getUpdates`, `sendMessage`, `editMessageText`, `sendChatAction`, `setMessageReaction`, and `setMyCommands` are all implemented. The `AgentLoop` supports streaming via `StreamCallback` and tool progress via `ToolProgressCallback`. The `SessionStore` and `GatewaySession` types are complete. The `GatewayRunner` skeleton exists. The gap is the async scaffolding that connects these pieces safely across concurrent Tokio tasks.

The three core wiring problems are: (1) replacing the existing `AtomicBool + handle.abort()` shutdown with `CancellationToken`-based cooperative shutdown, (2) replacing the current unbounded `tokio::spawn` per message with a `Semaphore`-bounded `JoinSet` supervisor, and (3) building a `StreamConsumer` that bridges `AgentLoop`'s `StreamCallback` to throttled `editMessageText` calls with cursor indicator and 4096-char overflow handling. The fourth significant piece is the per-user message queue (D-21/D-22) to serialize overlapping requests.

Beyond the async core, this phase also adds: multimodal input handling (images as vision content, PDFs via text extraction), user whitelist enforcement, slash command processing, session timeout expiry, and a gateway binary entry point. The `MessageHandler` trait needs redesign — its current `async fn handle(&self, event: &MessageEvent) -> Result<String>` return type cannot support streaming. The new handler must accept an `Arc<dyn PlatformAdapter>` (or equivalent) to push progressive edits directly.

**Primary recommendation:** Build in this order: (1) CancellationToken + Arc<RwLock<SessionStore>> refactor, (2) channel-based polling dispatcher with per-user queue, (3) StreamConsumer, (4) Semaphore-bounded JoinSet supervisor, (5) whitelist + slash commands, (6) multimodal input, (7) gateway binary. Each step is independently testable.

---

## Standard Stack

### Core
| Library | Version | Purpose | Why Standard |
|---------|---------|---------|--------------|
| tokio | 1 (workspace) | Async runtime, mpsc channels, Semaphore, JoinSet | Already in workspace |
| tokio-util | 0.7.18 | CancellationToken for cooperative shutdown | Standard complement to tokio for structured cancellation |
| reqwest | 0.12 (workspace) | HTTP client for Telegram Bot API | Already in workspace; all API calls use it |
| serde_json | 1 (workspace) | Telegram API request/response serialization | Already in workspace |
| tracing | 0.1 (workspace) | Structured logging per agent run | Already in workspace |
| uuid | 1 (workspace) | Session IDs | Already in workspace |
| chrono | 0.4 (workspace) | Session timeout timestamps | Already in workspace |

### Supporting
| Library | Version | Purpose | When to Use |
|---------|---------|---------|-------------|
| pdf-extract | 0.10.0 | PDF text extraction (D-07) | When processing document attachments |
| tokio::sync::RwLock | (tokio) | Arc<RwLock<SessionStore>> | Wrap SessionStore for cross-task sharing |
| tokio::task::JoinSet | (tokio) | Supervisor tracking active agent runs | TG-05, ASYNC-03 |
| tokio::sync::Semaphore | (tokio) | Bound concurrent agent runs | TG-06 |

### Alternatives Considered
| Instead of | Could Use | Tradeoff |
|------------|-----------|----------|
| pdf-extract | lopdf 0.40.0 | lopdf is lower-level; pdf-extract wraps it with text extraction. Use pdf-extract unless precise control needed. |
| hand-rolled Telegram client | teloxide | Decision locked: keep hand-rolled client. Zero new async framework dependencies. |
| mpsc per-user queue | broadcast channel | mpsc is correct: one consumer per chat. broadcast would be wasteful. |

**Installation (additions to workspace):**
```toml
# Workspace Cargo.toml — add:
tokio-util = { version = "0.7", features = ["rt"] }
pdf-extract = "0.10"

# ironhermes-gateway/Cargo.toml — add:
tokio-util = { workspace = true }
pdf-extract = { workspace = true }
```

**Version verification:** tokio-util 0.7.18 confirmed via `cargo search tokio-util` on 2026-04-01. pdf-extract 0.10.0 confirmed via `cargo search pdf-extract` on 2026-04-01.

---

## Architecture Patterns

### Recommended Project Structure
```
crates/ironhermes-gateway/src/
├── lib.rs              # pub re-exports
├── adapter.rs          # PlatformAdapter + MessageHandler traits (updated)
├── runner.rs           # GatewayRunner with CancellationToken + JoinSet (rewritten)
├── session.rs          # SessionStore wrapped in Arc<RwLock> (updated)
├── telegram.rs         # TelegramAdapter (updated: CancellationToken, setMyCommands)
├── stream_consumer.rs  # StreamConsumer: throttled edit loop (new)
├── user_queue.rs       # Per-user message queue, overlapping request serialization (new)
├── backoff.rs          # BackoffState: exponential backoff with jitter (new)
└── handler.rs          # GatewayMessageHandler: AgentLoop wiring (new)

crates/ironhermes-cli/src/
└── main.rs             # Add `gateway` subcommand dispatching to GatewayRunner
```

### Pattern 1: CancellationToken-Based Cooperative Shutdown (TG-05)

**What:** Replace `Arc<AtomicBool>` + `handle.abort()` with `tokio_util::sync::CancellationToken`. The token is cloned into every subtask; each task checks `token.cancelled()` or `select!`s on `token.cancelled()`.

**When to use:** Everywhere a task needs to stop cleanly when the gateway shuts down. Ctrl+C sets the token; all tasks observe it cooperatively.

```rust
// Source: tokio-util 0.7 docs + tokio select! pattern
use tokio_util::sync::CancellationToken;

let token = CancellationToken::new();

// Polling task
let poll_token = token.clone();
tokio::spawn(async move {
    loop {
        tokio::select! {
            _ = poll_token.cancelled() => break,
            result = fetch_updates(&http, &token_str, &mut offset) => {
                // handle result
            }
        }
    }
});

// Shutdown handler
tokio::spawn(async move {
    tokio::signal::ctrl_c().await.ok();
    token.cancel();  // broadcasts to all clones
});
```

### Pattern 2: Channel-Based Message Dispatch (TG-01)

**What:** Long-polling loop sends `TgMessage` values into an `mpsc::Sender<TgMessage>`. A dispatcher task receives from the channel and routes to per-user queues. This decouples network I/O from processing.

**When to use:** Core architecture for TG-01. Avoids nested `tokio::spawn` inside the polling loop.

```rust
// Source: tokio mpsc pattern
let (tx, mut rx) = tokio::sync::mpsc::channel::<TgMessage>(256);

// Polling task: sends to channel
tokio::spawn(async move {
    // ... polling loop ...
    for msg in updates {
        let _ = tx.send(msg).await;
    }
});

// Dispatcher task: receives and routes
tokio::spawn(async move {
    while let Some(msg) = rx.recv().await {
        dispatcher.route(msg).await;
    }
});
```

### Pattern 3: Per-User Message Queue (D-21, D-22)

**What:** `HashMap<chat_id, mpsc::Sender<TgMessage>>` where each chat_id has a single-consumer queue. A per-chat worker task processes messages sequentially. Overlapping messages get a 👀 reaction before being queued.

**When to use:** Enforces D-21 — overlapping messages from same user are queued, not dropped or parallelised.

```rust
// Source: pattern derived from existing add_reaction API in TelegramAdapter
struct UserQueue {
    queues: HashMap<String, mpsc::Sender<QueuedMessage>>,
}

struct QueuedMessage {
    event: MessageEvent,
    is_queued: bool,  // true if a previous run was in-flight at enqueue time
}

// When enqueuing:
// 1. If sender exists and channel is non-empty: add_reaction(chat_id, msg_id, "👀")
// 2. Send to existing sender or create new worker
```

### Pattern 4: StreamConsumer with Throttled Edits (TG-03)

**What:** A task that receives `StreamEvent::ContentDelta` values from `AgentLoop`'s stream callback via an `mpsc` channel, accumulates text, and calls `editMessageText` at most once per 300ms. Appends `█` cursor during generation, strips it on final edit. Handles 4096-char overflow by chaining a new message.

**When to use:** The bridge between AgentLoop streaming output and Telegram's edit API.

```rust
// Source: pattern from CONTEXT.md architecture notes
pub struct StreamConsumer {
    adapter: Arc<TelegramAdapter>,
    chat_id: String,
    message_id: String,         // ID of the placeholder "..." message
    buffer: String,             // accumulated text so far
    last_edit: Instant,
    edit_interval: Duration,    // 300ms
    overflow_messages: Vec<String>,  // IDs of chained overflow messages
}

impl StreamConsumer {
    pub async fn flush(&mut self, final_edit: bool) -> Result<()> {
        let now = Instant::now();
        if !final_edit && now.duration_since(self.last_edit) < self.edit_interval {
            return Ok(());
        }

        let display = if final_edit {
            self.buffer.clone()  // no cursor; apply parse_mode: Markdown
        } else {
            format!("{}\u{2588}", self.buffer)  // append █
        };

        // Handle 4096-char overflow: split at last paragraph break before limit
        if display.len() > 4096 {
            let split_point = find_split_point(&display, 4096);
            let (first, rest) = display.split_at(split_point);
            self.adapter.edit_message(&self.chat_id, &self.message_id, first).await?;
            // Send new message for continuation, update self.message_id
            let new_msg = self.adapter.send_message(&self.chat_id, rest, None).await?;
            self.message_id = new_msg.message_id;
        } else {
            self.adapter.edit_message(&self.chat_id, &self.message_id, &display).await?;
        }
        self.last_edit = now;
        Ok(())
    }
}
```

### Pattern 5: JoinSet Supervisor with Semaphore (TG-05, TG-06, ASYNC-03)

**What:** `tokio::task::JoinSet` holds all active agent run handles. `tokio::sync::Semaphore` limits concurrency to 4-8. On CancellationToken fire, drain the JoinSet (wait for all tasks to finish naturally).

**When to use:** Core concurrency control for TG-06 and graceful shutdown for TG-05.

```rust
// Source: tokio JoinSet + Semaphore pattern
let semaphore = Arc::new(Semaphore::new(MAX_CONCURRENT_RUNS));  // default: 8
let mut join_set: JoinSet<()> = JoinSet::new();

// Per message dispatch:
let permit = semaphore.clone().acquire_owned().await?;
join_set.spawn(async move {
    let _permit = permit;  // dropped when task ends, releasing slot
    run_agent_for_message(event, ...).await;
});

// Shutdown drain:
token.cancel();
while join_set.join_next().await.is_some() {}  // wait for all in-flight runs
```

### Pattern 6: BackoffState for Polling Error Recovery (TG-07)

**What:** Struct tracking consecutive failure count, computes next sleep duration as `min(base * 2^n + jitter, cap)`. Resets on success. Detects 409 conflicts specifically (fatal after 5 retries — indicates another bot instance is running).

**When to use:** Wraps the outer polling loop error handler.

```rust
// Source: pattern from CONTEXT.md architecture notes
pub struct BackoffState {
    base_ms: u64,       // 1000
    cap_ms: u64,        // 60_000
    failures: u32,
}

impl BackoffState {
    pub fn next_delay(&self) -> Duration {
        let exp = self.base_ms * (1u64 << self.failures.min(10));
        let jitter = rand::random::<u64>() % (exp / 4 + 1);
        Duration::from_millis((exp + jitter).min(self.cap_ms))
    }

    pub fn record_success(&mut self) { self.failures = 0; }
    pub fn record_failure(&mut self) { self.failures += 1; }
    pub fn is_fatal_conflict(&self) -> bool { self.failures >= 5 }
}
```

### Pattern 7: MessageHandler Trait Redesign (TG-03)

**What:** The current `MessageHandler::handle` returns `Result<String>` — incompatible with streaming. The new handler receives an `Arc<TelegramAdapter>` (or `Arc<dyn PlatformAdapter>`) so it can send the initial placeholder message, create a `StreamConsumer`, and push edits directly.

**The existing pattern that must change:**
```rust
// CURRENT (adapter.rs) — cannot support streaming:
async fn handle(&self, event: &MessageEvent) -> Result<String>;

// NEW — handler owns the adapter reference and drives edits directly:
async fn handle(
    &self,
    event: &MessageEvent,
    adapter: Arc<TelegramAdapter>,
    cancel: CancellationToken,
) -> Result<()>;
```

### Pattern 8: Typing Indicator Task (TG-08, D-16)

**What:** Spawn a separate task per agent run that calls `sendChatAction("typing")` every 5 seconds. Cancel it via a child `CancellationToken` when the agent run ends.

```rust
// Source: derived from TelegramAdapter.api_call pattern
let typing_token = cancel.child_token();
let typing_handle = {
    let adapter = adapter.clone();
    let chat_id = event.chat_id.clone();
    let token = typing_token.clone();
    tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = token.cancelled() => break,
                _ = tokio::time::sleep(Duration::from_secs(5)) => {
                    let _ = adapter.send_chat_action(&chat_id, "typing").await;
                }
            }
        }
    })
};
// After agent run ends:
typing_token.cancel();
let _ = typing_handle.await;
```

### Pattern 9: Gateway Binary Entry Point

**What:** Add a `gateway` subcommand to the existing `ironhermes` CLI binary. This avoids a separate binary and reuses the existing `Config::load()` + `ToolRegistry::register_defaults()` setup pattern from `main.rs`.

```rust
// In crates/ironhermes-cli/src/main.rs — add to Commands enum:
Gateway {
    /// Override Telegram bot token
    #[arg(long)]
    token: Option<String>,
}

// Handler mirrors run_chat() setup, then calls GatewayRunner::start()
```

### Pattern 10: Multimodal Input — Images (D-06)

**What:** When a `TgMessage` contains `photo` (array of `PhotoSize`), download the largest size via `getFile` + `https://api.telegram.org/file/bot{token}/{file_path}`, then encode as base64 and build a `ContentPart::ImageUrl { image_url: ImageUrl { url: "data:image/jpeg;base64,..." } }` message part alongside any caption text.

**Key Telegram API facts:**
- `getFile` returns a `File` object with `file_path`
- Download URL: `https://api.telegram.org/file/bot{token}/{file_path}`
- Maximum bot download: 20MB (D-08)
- Vision models accept `image_url` with `data:` URI scheme

### Pattern 11: Multimodal Input — PDFs and Documents (D-07)

**What:** When a `TgMessage` contains `document`, download the file, detect MIME type. For `application/pdf`, use `pdf-extract` to extract text. For other documents (plain text, markdown), read bytes as UTF-8. Inject extracted text as a `[Document: filename]\n{content}` prefix to the user message.

```rust
// pdf-extract usage (verified: pdf-extract 0.10.0)
use pdf_extract::extract_text;

let text = extract_text(&file_bytes_as_cursor)?;
let context = format!("[Document: {}]\n{}", filename, text);
```

### Pattern 12: Config Extensions for Gateway

**What:** `GatewayConfig` and `PlatformGatewayConfig` in `ironhermes-core` need whitelist and session timeout fields. These are deserialized from `~/.ironhermes/config.yaml`.

```rust
// Add to PlatformGatewayConfig in ironhermes-core/src/config.rs:
pub whitelist: Vec<i64>,            // Telegram user IDs; empty = deny all (D-12)
pub session_timeout_hours: u64,    // default 24 (D-14)
pub max_concurrent_runs: usize,    // default 8 (TG-06)
```

### Anti-Patterns to Avoid

- **Unbounded tokio::spawn per message:** The existing polling loop does this. Every message spawns without limits. Replace with Semaphore-gated JoinSet.
- **Arc<Mutex> on SessionStore:** Use `Arc<RwLock<SessionStore>>` — session reads vastly outnumber writes; RwLock avoids read contention. Never hold the write lock across an `.await` boundary.
- **Holding RwLock across await points:** Extract needed data before any `.await` call. `let messages = { store.read().await.get(key).unwrap().messages.clone() };`
- **Calling editMessageText too frequently:** Telegram rate-limits edits to ~30/minute. StreamConsumer's 300ms throttle enforces this. Do not call edit on every ContentDelta.
- **Setting parse_mode during streaming:** Mid-stream Markdown formatting will be broken (incomplete bold/code spans). Only apply `parse_mode: "Markdown"` on the final edit (D-03).
- **Using handle.abort() for shutdown:** Aborts leave Telegram messages in "editing" state with `█` cursor. CancellationToken lets the StreamConsumer send the final clean edit first.

---

## Don't Hand-Roll

| Problem | Don't Build | Use Instead | Why |
|---------|-------------|-------------|-----|
| Cooperative shutdown signaling | Custom AtomicBool propagation | `tokio_util::sync::CancellationToken` | Propagates to child tasks; existing AtomicBool only covers the poll loop, not agent tasks |
| PDF text extraction | Custom PDF parser | `pdf-extract 0.10` | PDF format is notoriously complex; pdf-extract wraps lopdf with text reconstruction |
| Task lifecycle tracking | Vec<JoinHandle> + manual drain | `tokio::task::JoinSet` | JoinSet's `join_next()` and `abort_all()` are exactly the drain pattern needed |
| Concurrency limiting | Custom AtomicUsize counter | `tokio::sync::Semaphore` | Semaphore integrates naturally with async; permits auto-release on task drop |
| Exponential backoff | Custom sleep doubling | `BackoffState` (hand-roll simple struct) | Trivial enough to hand-roll (10 lines); external crate adds no value here |

**Key insight:** All async primitives needed (Semaphore, JoinSet, mpsc, RwLock, select!) are in tokio itself. The only external addition for the async core is `tokio-util` for `CancellationToken`.

---

## Common Pitfalls

### Pitfall 1: RwLock Deadlock Across Await Points
**What goes wrong:** Code takes a write lock on `SessionStore`, then calls an async function. The task yields, another task tries to acquire the same lock, deadlock occurs.
**Why it happens:** Tokio's `RwLock` is not reentrant. Holding a guard across `.await` is undefined behavior in safe code.
**How to avoid:** Extract all needed data before any `.await`. Use the pattern: `let data = { store.read().await.clone_needed_data() };` then drop the guard before `await`.
**Warning signs:** Code like `let guard = store.write().await; guard.update(); some_async_fn().await;`

### Pitfall 2: Telegram 409 Conflict (Multiple Bot Instances)
**What goes wrong:** Two instances of the bot poll simultaneously. Telegram returns HTTP 409 with "Conflict: terminated by other getUpdates request". The bot storms retries.
**Why it happens:** Long polling is exclusive — only one connection per token at a time.
**How to avoid:** Track 409 errors specifically in `BackoffState`. After 5 consecutive 409s, log a fatal error ("Another bot instance is running") and cancel the CancellationToken — full shutdown, not retry.
**Warning signs:** Rapid-fire 409 errors in logs, polling never succeeds.

### Pitfall 3: Telegram 429 Rate Limiting on Edit Spam
**What goes wrong:** StreamConsumer sends an edit on every ContentDelta. Telegram returns 429 Too Many Requests with `retry_after` field. Messages stop updating.
**Why it happens:** Telegram limits `editMessageText` to ~20 per minute per bot across all chats combined.
**How to avoid:** StreamConsumer enforces 300ms minimum between edits. On 429, parse `retry_after` from response and sleep that duration (D-19 covers this for send; same logic applies to edits).
**Warning signs:** `retry_after` field in Telegram error responses, editing stops mid-stream.

### Pitfall 4: Streaming Callback Is Not Send + Sync
**What goes wrong:** The `StreamCallback` type is `Box<dyn Fn(&str) + Send + Sync>`. The closure captures `StreamConsumer` which holds an `Arc<TelegramAdapter>`. If `TelegramAdapter` is not `Send + Sync`, the whole chain fails to compile.
**Why it happens:** `TelegramAdapter` has a `reqwest::Client` (which is `Send + Sync`) but the callback closes over mutable state (the buffer). Mutable closures are not `Fn`, only `FnMut`.
**How to avoid:** Use `mpsc::Sender<String>` inside the callback instead of capturing mutable state. The closure becomes `Box<dyn Fn(&str) + Send + Sync>` — just sends to channel. `StreamConsumer` runs as a separate task receiving from that channel.
**Warning signs:** Compiler errors: `the trait Send is not implemented for...`, `cannot borrow as mutable`.

### Pitfall 5: Slash Command Text Passes to Agent Loop
**What goes wrong:** `/start`, `/new`, `/clear`, `/help` are processed as normal user messages. The agent responds with confused output instead of executing the command.
**Why it happens:** tg_message_to_event() maps message.text directly to event.content. No slash command interception.
**How to avoid:** In the handler, check if `event.content.starts_with('/')` before routing to agent. Parse the command and dispatch to the appropriate handler function first.
**Warning signs:** Agent responds to "/new" with "I'm not sure what you mean by /new..."

### Pitfall 6: File Download Blocks the Polling Loop
**What goes wrong:** Image/PDF download happens synchronously in the dispatch path. The polling loop stalls while waiting for a 20MB file download.
**Why it happens:** Downloads are async but if awaited in the wrong task, they block the dispatcher.
**How to avoid:** File downloads happen inside the per-message agent task (after dispatch via channel), never in the polling loop or dispatcher task.
**Warning signs:** Other messages queue up and are not processed while a file is downloading.

### Pitfall 7: Session Timeout Not Checked on Write
**What goes wrong:** Stale sessions accumulate in memory. GatewaySession.updated_at is set but never checked. Memory grows unbounded.
**Why it happens:** SessionStore.get_or_create() creates new sessions but `get_or_create` never evicts expired ones.
**How to avoid:** Add `expire_stale()` method to SessionStore called periodically (e.g., in the dispatcher task every 5 minutes). Remove sessions where `updated_at` is older than `session_timeout_hours`.
**Warning signs:** Memory growth proportional to number of unique users over time.

---

## Code Examples

### StreamConsumer Integration with AgentLoop Streaming

The key insight: `AgentLoop::with_streaming` takes `Box<dyn Fn(&str) + Send + Sync>`. The callback cannot be async and cannot hold a mutable buffer. Solution: use a channel. The callback sends text chunks; a separate task drives the `StreamConsumer`.

```rust
// Source: derived from AgentLoop::with_streaming in ironhermes-agent/src/agent_loop.rs
let (stream_tx, mut stream_rx) = tokio::sync::mpsc::channel::<String>(256);
let stream_tx_clone = stream_tx.clone();

// This satisfies: Box<dyn Fn(&str) + Send + Sync>
let stream_callback = Box::new(move |delta: &str| {
    let _ = stream_tx_clone.try_send(delta.to_string());
});

// Separate task drives StreamConsumer
let adapter_clone = adapter.clone();
let consumer_handle = tokio::spawn(async move {
    let mut consumer = StreamConsumer::new(adapter_clone, chat_id, placeholder_msg_id);
    while let Some(chunk) = stream_rx.recv().await {
        consumer.push(&chunk);
        consumer.flush(false).await.ok();
    }
    consumer.flush(true).await.ok();  // final edit: strips cursor, applies Markdown
});

let agent = AgentLoop::new(client, registry, max_turns)
    .with_streaming(stream_callback)
    .with_tool_progress(Box::new(move |name, _args| {
        let _ = tool_tx.try_send(format!("⚙️ Running: {}...", name));
    }));

let result = agent.run(messages).await?;
drop(stream_tx);  // closes channel; consumer_handle finishes
consumer_handle.await?;
```

### GatewayRunner Start with CancellationToken

```rust
// Source: derived from runner.rs skeleton + tokio-util CancellationToken API
pub struct GatewayRunner {
    config: Config,
    session_store: Arc<RwLock<SessionStore>>,
    tool_registry: Arc<ToolRegistry>,
    cancel: CancellationToken,
}

impl GatewayRunner {
    pub async fn start(&self) -> Result<()> {
        let token = resolve_telegram_token(&self.config)?;
        let adapter = Arc::new(TelegramAdapter::new(token));

        // Register commands on startup (D-17)
        adapter.set_my_commands(&SLASH_COMMANDS).await?;

        let (msg_tx, msg_rx) = mpsc::channel::<TgMessage>(256);

        // Polling task
        let poll_cancel = self.cancel.clone();
        tokio::spawn(poll_loop(adapter.clone(), msg_tx, poll_cancel));

        // Dispatcher task
        let dispatch_cancel = self.cancel.clone();
        tokio::spawn(dispatch_loop(
            adapter.clone(),
            msg_rx,
            self.session_store.clone(),
            self.tool_registry.clone(),
            self.config.clone(),
            dispatch_cancel,
        ));

        // Wait for shutdown signal
        tokio::signal::ctrl_c().await?;
        info!("Shutdown signal received, draining...");
        self.cancel.cancel();

        Ok(())
    }
}
```

### Whitelist Check

```rust
// Source: derived from D-10, D-11, D-12 in CONTEXT.md
fn is_authorized(event: &MessageEvent, whitelist: &[i64]) -> bool {
    if whitelist.is_empty() {
        return false;  // D-12: empty = deny all
    }
    let sender_id: i64 = event.sender_id.parse().unwrap_or(0);
    whitelist.contains(&sender_id)
}
// D-11: checked before any processing, for both DMs and group @mentions
// D-11: unauthorized users silently ignored (no response, no reaction)
```

### Group @mention Filter

```rust
// Source: derived from D-09 and existing tg_message_to_event() in telegram.rs
fn should_process(event: &MessageEvent, bot_username: &str) -> bool {
    if event.chat_type == "dm" {
        return true;  // Always process DMs
    }
    // Group/supergroup: only process @mentions
    let mention = format!("@{}", bot_username);
    event.content.contains(&mention)
}
```

### Session Timeout Eviction

```rust
// Source: derived from GatewaySession fields in session.rs
impl SessionStore {
    pub fn expire_stale(&mut self, timeout_hours: u64) {
        let cutoff = Utc::now() - chrono::Duration::hours(timeout_hours as i64);
        self.sessions.retain(|_, session| session.updated_at > cutoff);
    }
}
// Called periodically — not on every message — to avoid write lock contention
```

---

## State of the Art

| Old Approach | Current Approach | When Changed | Impact |
|--------------|------------------|--------------|--------|
| AtomicBool + handle.abort() | CancellationToken cooperative shutdown | tokio-util 0.7 | Clean graceful shutdown; tasks can send final Telegram edits before stopping |
| tokio::spawn unbounded | JoinSet + Semaphore bounded | tokio 1.x | Prevents resource exhaustion under load |
| Box<dyn MessageHandler> (non-streaming) | Handler takes Arc<Adapter> and drives edits directly | This phase | Enables streaming TG-03 requirement |

**Deprecated/outdated in existing code:**
- `Arc<AtomicBool>` in `TelegramAdapter`: replace with `CancellationToken`
- `handle.abort()` in `TelegramAdapter::stop()`: replace with `cancel.cancel()` + join
- `Box<dyn MessageHandler>` passed to `PlatformAdapter::start()`: replace with handler that holds `Arc<TelegramAdapter>` directly
- Inline `tokio::spawn` in polling loop body: replace with channel dispatch

---

## Open Questions

1. **Vision model for images (D-06)**
   - What we know: `config.model.vision_model` field exists in `ModelConfig`; `ContentPart::ImageUrl` type exists in `ironhermes-core`
   - What's unclear: Does the configured default model support vision, or must we switch to `vision_model` for image messages?
   - Recommendation: Check if `vision_model` is set in config; if so, use it for image messages. If not, use the default model (most modern models support vision). Document in config YAML example.

2. **rand crate for backoff jitter**
   - What we know: `BackoffState` needs random jitter; `rand` is not in workspace dependencies
   - What's unclear: Is it worth adding `rand` to workspace, or use a simpler approach?
   - Recommendation: Use `std::time::SystemTime::now().duration_since(UNIX_EPOCH).unwrap().subsec_nanos() % max_jitter` as a zero-dependency jitter source. Acceptable for backoff.

3. **`getFile` API type additions to TelegramAdapter**
   - What we know: `TelegramAdapter` has `TgMessage` struct but no `photo`, `document`, or `TgFile` fields
   - What's unclear: Exact Telegram API response shape for `getFile`
   - Recommendation: Add `photo: Option<Vec<TgPhotoSize>>` and `document: Option<TgDocument>` to `TgMessage`. Add `TgFile { file_id, file_path, file_size }` type. All verified against Telegram Bot API docs (stable, unchanged since Bot API 3.0).

---

## Environment Availability

| Dependency | Required By | Available | Version | Fallback |
|------------|------------|-----------|---------|----------|
| Rust/Cargo | Build | ✓ | (workspace uses edition 2024) | — |
| tokio (full) | Async runtime | ✓ | 1.x (workspace) | — |
| reqwest 0.12 | HTTP/Telegram API | ✓ | 0.12 (workspace) | — |
| tokio-util | CancellationToken | Must add | 0.7.18 | AtomicBool fallback (inferior, avoid) |
| pdf-extract | PDF text extraction | Must add | 0.10.0 | Skip PDF support until added |
| Telegram Bot Token | Live bot testing | Runtime config | — | Integration tests require real token |
| TELEGRAM_BOT_TOKEN env var | Gateway binary | Runtime | — | Config file `~/.ironhermes/config.yaml` |

**Missing dependencies with no fallback:**
- `tokio-util` must be added to workspace — `CancellationToken` is the locked shutdown mechanism

**Missing dependencies with fallback:**
- `pdf-extract` can be deferred — PDF support (D-07) is independent of the core async wiring. Bot works without it; documents are declined gracefully.

---

## Validation Architecture

### Test Framework
| Property | Value |
|----------|-------|
| Framework | Rust built-in `#[test]` + `#[tokio::test]` |
| Config file | None needed — workspace uses `cargo test` |
| Quick run command | `cargo test -p ironhermes-gateway` |
| Full suite command | `cargo test --workspace` |

### Phase Requirements → Test Map

| Req ID | Behavior | Test Type | Automated Command | File Exists? |
|--------|----------|-----------|-------------------|-------------|
| TG-01 | Polling loop dispatches messages to handler | unit (mock HTTP) | `cargo test -p ironhermes-gateway test_poll_dispatches` | ❌ Wave 0 |
| TG-02 | Agent response sent to originating chat | integration | `cargo test -p ironhermes-gateway test_response_routing` | ❌ Wave 0 |
| TG-03 | StreamConsumer emits edits with cursor, strips on final | unit | `cargo test -p ironhermes-gateway test_stream_consumer` | ❌ Wave 0 |
| TG-04 | Session created on first message, reused on second | unit | `cargo test -p ironhermes-gateway test_session_lifecycle` | ❌ Wave 0 |
| TG-05 | CancellationToken stops poll loop; JoinSet drains | unit | `cargo test -p ironhermes-gateway test_graceful_shutdown` | ❌ Wave 0 |
| TG-06 | Semaphore blocks >8 concurrent spawns | unit | `cargo test -p ironhermes-gateway test_semaphore_limit` | ❌ Wave 0 |
| TG-07 | BackoffState doubles delay, caps at 60s, adds jitter | unit | `cargo test -p ironhermes-gateway test_backoff_state` | ❌ Wave 0 |
| TG-08 | Typing indicator task sends every 5s, stops on cancel | unit | `cargo test -p ironhermes-gateway test_typing_indicator` | ❌ Wave 0 |
| ASYNC-01 | SessionStore readable from two concurrent tasks | unit | `cargo test -p ironhermes-gateway test_session_store_concurrent` | ❌ Wave 0 |
| ASYNC-02 | ToolRegistry shared across agent tasks compiles and runs | unit | `cargo test -p ironhermes-gateway test_tool_registry_shared` | ❌ Wave 0 |
| ASYNC-03 | Polling loop restarts after transient error | unit | `cargo test -p ironhermes-gateway test_poll_restart_on_error` | ❌ Wave 0 |

**Note on TG-01/TG-02/ASYNC-03:** Full integration tests require a real Telegram Bot Token. Unit tests mock HTTP responses using `wiremock` or `mockall`. Manual smoke test against real bot validates end-to-end.

### Sampling Rate
- **Per task commit:** `cargo test -p ironhermes-gateway`
- **Per wave merge:** `cargo test --workspace`
- **Phase gate:** Full workspace green + manual bot smoke test before `/gsd:verify-work`

### Wave 0 Gaps
- [ ] `crates/ironhermes-gateway/src/stream_consumer.rs` — covers TG-03
- [ ] `crates/ironhermes-gateway/src/backoff.rs` — covers TG-07
- [ ] `crates/ironhermes-gateway/src/user_queue.rs` — covers D-21/D-22
- [ ] `crates/ironhermes-gateway/src/handler.rs` — covers TG-01, TG-02, TG-08
- [ ] `crates/ironhermes-gateway/tests/` — integration test directory
- [ ] Add `tokio-util` to workspace Cargo.toml — required for CancellationToken (TG-05)

---

## Sources

### Primary (HIGH confidence)
- Codebase inspection — `crates/ironhermes-gateway/src/telegram.rs`, `adapter.rs`, `runner.rs`, `session.rs`
- Codebase inspection — `crates/ironhermes-agent/src/agent_loop.rs`, `client.rs`
- Codebase inspection — `crates/ironhermes-core/src/types.rs`, `config.rs`
- Codebase inspection — `crates/ironhermes-cli/src/main.rs` (agent setup pattern reference)
- `.planning/phases/02-telegram-gateway/02-CONTEXT.md` — locked decisions
- `.planning/ROADMAP.md` §Phase 2 — key technical decisions

### Secondary (MEDIUM confidence)
- `cargo search tokio-util` (2026-04-01) — confirmed version 0.7.18
- `cargo search pdf-extract` (2026-04-01) — confirmed version 0.10.0
- Telegram Bot API docs (stable API, unchanged since Bot API 3.0+): getUpdates, sendMessage, editMessageText, sendChatAction, setMessageReaction, setMyCommands, getFile

### Tertiary (LOW confidence)
- BackoffState jitter parameters (1s base, 60s cap) — from CONTEXT.md/ROADMAP.md notes, not independently benchmarked
- 300ms StreamConsumer edit interval — from architecture notes; Telegram's actual rate limit is ~20 edits/minute globally, so 300ms per-message is conservative and safe

---

## Metadata

**Confidence breakdown:**
- Standard stack: HIGH — all dependencies confirmed via cargo search and workspace inspection
- Architecture: HIGH — derived directly from existing code; no speculative abstractions
- Pitfalls: HIGH — based on direct inspection of existing code anti-patterns (unbounded spawn, AtomicBool abort, missing whitelist)
- Multimodal patterns: MEDIUM — Telegram file download API is stable but not verified against live bot

**Research date:** 2026-04-01
**Valid until:** 2026-05-01 (Telegram Bot API is stable; tokio ecosystem changes slowly)
