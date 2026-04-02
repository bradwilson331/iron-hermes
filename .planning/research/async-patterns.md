# Rust Async Patterns for Long-Running AI Agent Services

**Project:** IronHermes
**Researched:** 2026-04-01
**Confidence:** MEDIUM (training data only; web search/docs unavailable for live verification)

---

## Table of Contents

1. [Structured Concurrency with Tokio](#1-structured-concurrency-with-tokio)
2. [Channel Patterns](#2-channel-patterns)
3. [Error Handling in Long-Running Services](#3-error-handling-in-long-running-services)
4. [Shared State Patterns](#4-shared-state-patterns)
5. [Backpressure and Rate Limiting](#5-backpressure-and-rate-limiting)
6. [Testing Async Code](#6-testing-async-code)
7. [Recommendations for IronHermes](#7-recommendations-for-ironhermes)

---

## 1. Structured Concurrency with Tokio

### Problem in IronHermes Today

The current `TelegramAdapter::start()` uses raw `tokio::spawn` for both the polling loop and per-message handler dispatch (lines 94 and 123 in `telegram.rs`). These spawned tasks are fire-and-forget -- there is no tracking, no cancellation propagation, and no way to know when all in-flight agent runs have drained during shutdown. The `stop()` method calls `handle.abort()` which kills the poll loop immediately, potentially orphaning active agent runs mid-execution.

### JoinSet for Task Group Management

`tokio::task::JoinSet` is the primary structured concurrency primitive. It tracks a group of spawned tasks and lets you await them collectively.

```rust
use tokio::task::JoinSet;

struct AgentSupervisor {
    active_runs: JoinSet<Result<AgentResult, anyhow::Error>>,
    max_concurrent: usize,
}

impl AgentSupervisor {
    fn new(max_concurrent: usize) -> Self {
        Self {
            active_runs: JoinSet::new(),
            max_concurrent,
        }
    }

    /// Spawn a new agent run, tracked by the JoinSet.
    fn spawn_agent_run(
        &mut self,
        agent: Arc<AgentLoop>,
        messages: Vec<ChatMessage>,
        chat_id: String,
    ) {
        self.active_runs.spawn(async move {
            let result = agent.run(messages).await?;
            Ok(result)
        });
    }

    /// Drain completed tasks, collecting results.
    /// Call this in the main select! loop to reap finished tasks.
    async fn reap_completed(&mut self) -> Option<(String, Result<AgentResult, anyhow::Error>)> {
        // join_next returns None when the set is empty
        match self.active_runs.join_next().await {
            Some(Ok(result)) => Some(("ok".into(), result)),
            Some(Err(join_err)) => {
                // JoinError means the task panicked or was cancelled
                tracing::error!("Agent task failed: {}", join_err);
                None
            }
            None => None, // No tasks in the set
        }
    }

    /// Graceful shutdown: wait for all active runs to complete with a timeout.
    async fn shutdown(mut self, timeout: Duration) {
        tracing::info!(
            active = self.active_runs.len(),
            "Shutting down, waiting for active agent runs"
        );

        let drain = async {
            while self.active_runs.join_next().await.is_some() {}
        };

        match tokio::time::timeout(timeout, drain).await {
            Ok(()) => tracing::info!("All agent runs completed"),
            Err(_) => {
                tracing::warn!(
                    remaining = self.active_runs.len(),
                    "Shutdown timeout, aborting remaining tasks"
                );
                self.active_runs.abort_all();
            }
        }
    }
}
```

**Key JoinSet methods:**
- `spawn()` -- add a task to the set
- `join_next().await` -- await the next completed task (returns `None` when empty)
- `len()` -- number of active tasks
- `abort_all()` -- cancel all tasks in the set
- `shutdown().await` -- abort all and wait for them to finish (cleaner than abort_all)

### CancellationToken for Cooperative Shutdown

`tokio_util::sync::CancellationToken` is the standard pattern for propagating shutdown signals through a task tree. It replaces the `AtomicBool` pattern currently used in `TelegramAdapter`.

**Add dependency:** `tokio-util = { version = "0.7", features = ["rt"] }`

```rust
use tokio_util::sync::CancellationToken;

struct GatewayService {
    token: CancellationToken,
}

impl GatewayService {
    fn new() -> Self {
        Self {
            token: CancellationToken::new(),
        }
    }

    async fn run(&self) {
        // Create child tokens for subsystems -- cancelling parent cancels children
        let poll_token = self.token.child_token();
        let agent_token = self.token.child_token();

        let poll_handle = tokio::spawn(Self::poll_loop(poll_token));
        let agent_handle = tokio::spawn(Self::agent_processor(agent_token));

        // Wait for shutdown signal
        tokio::signal::ctrl_c().await.unwrap();
        tracing::info!("Shutdown signal received");

        // Cancel everything in the tree
        self.token.cancel();

        // Wait for orderly shutdown
        let _ = tokio::join!(poll_handle, agent_handle);
    }

    async fn poll_loop(token: CancellationToken) {
        loop {
            tokio::select! {
                _ = token.cancelled() => {
                    tracing::info!("Poll loop shutting down");
                    break;
                }
                result = Self::poll_updates() => {
                    match result {
                        Ok(updates) => { /* process */ }
                        Err(e) => {
                            tracing::warn!("Poll error: {}", e);
                            tokio::time::sleep(Duration::from_secs(5)).await;
                        }
                    }
                }
            }
        }
    }

    async fn poll_updates() -> Result<Vec<Update>, anyhow::Error> {
        // Long-poll Telegram API
        todo!()
    }

    async fn agent_processor(token: CancellationToken) {
        // Each agent run gets a child token so it can be individually cancelled
        let mut runs = JoinSet::new();

        loop {
            tokio::select! {
                _ = token.cancelled() => {
                    tracing::info!("Agent processor shutting down, draining {} runs", runs.len());
                    // Let active runs finish with a timeout
                    let drain = async { while runs.join_next().await.is_some() {} };
                    let _ = tokio::time::timeout(Duration::from_secs(30), drain).await;
                    runs.abort_all();
                    break;
                }
                Some(result) = runs.join_next() => {
                    // Reap completed agent runs
                    match result {
                        Ok(Ok(agent_result)) => { /* deliver response */ }
                        Ok(Err(e)) => tracing::error!("Agent run failed: {}", e),
                        Err(e) => tracing::error!("Agent task panicked: {}", e),
                    }
                }
            }
        }
    }
}
```

**CancellationToken vs AtomicBool (current pattern):**

| Aspect | AtomicBool | CancellationToken |
|--------|-----------|-------------------|
| Wakeup | Must poll in a loop | `cancelled()` is a future, wakes via `select!` |
| Hierarchy | Flat, manual propagation | `child_token()` creates a tree |
| select! integration | Needs manual check | First-class `select!` branch |
| Ergonomics | Low-level | Purpose-built for shutdown |

**Recommendation:** Replace `Arc<AtomicBool>` in `TelegramAdapter` with `CancellationToken`. This eliminates the busy-polling shutdown check and integrates cleanly with `select!`.

### The Main Service Loop Pattern

The idiomatic tokio pattern for a long-running service with multiple subsystems:

```rust
async fn main() -> Result<()> {
    let token = CancellationToken::new();
    let mut set = JoinSet::new();

    // Spawn subsystems, each getting a child token
    set.spawn(telegram_poller(token.child_token(), tx.clone()));
    set.spawn(agent_dispatcher(token.child_token(), rx, response_tx));
    set.spawn(response_deliverer(token.child_token(), response_rx));
    set.spawn(cron_scheduler(token.child_token()));

    // Wait for first exit or ctrl+c
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {
            tracing::info!("Ctrl+C received");
        }
        Some(result) = set.join_next() => {
            // A subsystem exited unexpectedly
            tracing::error!("Subsystem exited: {:?}", result);
        }
    }

    // Initiate graceful shutdown
    token.cancel();

    // Give subsystems time to drain
    let shutdown_timeout = Duration::from_secs(30);
    let drain = async { while set.join_next().await.is_some() {} };
    match tokio::time::timeout(shutdown_timeout, drain).await {
        Ok(()) => tracing::info!("Clean shutdown"),
        Err(_) => {
            tracing::warn!("Forced shutdown after timeout");
            set.abort_all();
        }
    }

    Ok(())
}
```

---

## 2. Channel Patterns

### Architecture: Polling -> Dispatch -> Delivery Pipeline

IronHermes needs a three-stage pipeline:
1. **Telegram poller** produces `MessageEvent`s
2. **Agent dispatcher** consumes events, runs agent loops, produces responses
3. **Response deliverer** sends responses back to Telegram

Channels decouple these stages, enabling backpressure and independent error handling.

### mpsc: The Workhorse Channel

`tokio::sync::mpsc` is bounded, multi-producer single-consumer. Use it for the main message pipeline.

```rust
use tokio::sync::mpsc;

// Bounded channel provides backpressure.
// If the agent dispatcher falls behind, the poller blocks on send.
// Buffer size = max queued messages before backpressure kicks in.
let (event_tx, event_rx) = mpsc::channel::<IncomingMessage>(64);
let (response_tx, response_rx) = mpsc::channel::<OutgoingResponse>(64);

/// What flows through the event channel.
#[derive(Debug)]
struct IncomingMessage {
    event: MessageEvent,
    /// Oneshot for the response -- enables direct request-response pairing
    response_tx: oneshot::Sender<String>,
}

/// What flows through the response channel (if not using oneshot).
#[derive(Debug)]
struct OutgoingResponse {
    chat_id: String,
    message_id: Option<String>,
    content: String,
    platform: Platform,
}

// --- Poller side ---
async fn telegram_poller(
    token: CancellationToken,
    tx: mpsc::Sender<IncomingMessage>,
) {
    let mut offset: Option<i64> = None;

    loop {
        tokio::select! {
            _ = token.cancelled() => break,
            result = poll_updates(&mut offset) => {
                match result {
                    Ok(updates) => {
                        for update in updates {
                            let (resp_tx, resp_rx) = oneshot::channel();
                            let msg = IncomingMessage {
                                event: update.into_event(),
                                response_tx: resp_tx,
                            };

                            // This will apply backpressure if the dispatcher is full.
                            // Use try_send to drop messages under overload instead.
                            if tx.send(msg).await.is_err() {
                                tracing::warn!("Dispatcher channel closed");
                                return;
                            }
                        }
                    }
                    Err(e) => {
                        tracing::warn!("Poll error: {}", e);
                        tokio::time::sleep(Duration::from_secs(5)).await;
                    }
                }
            }
        }
    }
}

// --- Dispatcher side ---
async fn agent_dispatcher(
    token: CancellationToken,
    mut rx: mpsc::Receiver<IncomingMessage>,
    agent: Arc<AgentLoop>,
    semaphore: Arc<Semaphore>,
) {
    let mut active_runs = JoinSet::new();

    loop {
        tokio::select! {
            _ = token.cancelled() => {
                // Drain active runs before exiting
                rx.close(); // Stop accepting new messages
                while active_runs.join_next().await.is_some() {}
                break;
            }
            Some(msg) = rx.recv() => {
                let agent = agent.clone();
                let permit = semaphore.clone().acquire_owned().await.unwrap();

                active_runs.spawn(async move {
                    let result = agent.run(vec![/* build messages */]).await;
                    let response = match result {
                        Ok(r) => r.final_response.unwrap_or_default(),
                        Err(e) => format!("Error: {}", e),
                    };
                    let _ = msg.response_tx.send(response);
                    drop(permit); // Release concurrency slot
                });
            }
            Some(result) = active_runs.join_next() => {
                if let Err(e) = result {
                    tracing::error!("Agent task panicked: {}", e);
                }
            }
        }
    }
}
```

### oneshot: Request-Response Pairing

`tokio::sync::oneshot` pairs a single request with its response. Embed it in the channel message to let the poller await its specific response.

```rust
use tokio::sync::oneshot;

// In the poller, for each message:
let (resp_tx, resp_rx) = oneshot::channel::<String>();
event_tx.send(IncomingMessage { event, response_tx: resp_tx }).await?;

// Spawn a task to await the response and send it back to Telegram
tokio::spawn(async move {
    match tokio::time::timeout(Duration::from_secs(120), resp_rx).await {
        Ok(Ok(response)) => {
            send_telegram_message(chat_id, &response).await;
        }
        Ok(Err(_)) => {
            // Sender dropped -- agent task was cancelled
            tracing::warn!("Agent run cancelled for chat {}", chat_id);
        }
        Err(_) => {
            // Timeout
            send_telegram_message(chat_id, "Request timed out.").await;
        }
    }
});
```

### watch: Shared Configuration / Status

`tokio::sync::watch` is a single-value channel where receivers always see the latest value. Use it for runtime configuration changes and service health status.

```rust
use tokio::sync::watch;

// Service health status -- all subsystems can read the latest state
let (health_tx, health_rx) = watch::channel(ServiceHealth::Starting);

// In a subsystem:
async fn monitor_health(mut rx: watch::Receiver<ServiceHealth>) {
    while rx.changed().await.is_ok() {
        let health = rx.borrow().clone();
        tracing::info!("Service health: {:?}", health);
    }
}

// Runtime model switching:
let (model_tx, model_rx) = watch::channel("nous/hermes-3".to_string());

// Agent reads current model before each run:
let current_model = model_rx.borrow().clone();
```

### broadcast: Fan-Out Events

`tokio::sync::broadcast` sends each message to all receivers. Use for event bus patterns where multiple subsystems need to observe the same events (logging, metrics, audit).

```rust
use tokio::sync::broadcast;

let (event_bus_tx, _) = broadcast::channel::<SystemEvent>(256);

// Multiple subscribers each get every event
let mut metrics_rx = event_bus_tx.subscribe();
let mut audit_rx = event_bus_tx.subscribe();

// Publish events from anywhere
event_bus_tx.send(SystemEvent::AgentRunStarted { chat_id: "123".into() })?;
```

**Warning:** broadcast channels drop old messages when the buffer is full (lagging receivers get `RecvError::Lagged`). Size the buffer generously or handle the lag error.

### Channel Selection Guide for IronHermes

| Channel | Use Case in IronHermes |
|---------|----------------------|
| `mpsc` | Telegram updates -> agent dispatcher, agent results -> response sender |
| `oneshot` | Pairing a specific incoming message with its agent response |
| `watch` | Current model config, service health status, rate limit state |
| `broadcast` | System-wide events (agent started, completed, error) for metrics/logging |

---

## 3. Error Handling in Long-Running Services

### Classification: When to Panic vs Recover

```rust
/// Error classification for the service supervisor.
enum ErrorSeverity {
    /// Retry the operation. Example: network timeout, 429 rate limit.
    Transient,
    /// Skip this item and continue. Example: malformed message, unknown tool.
    Permanent,
    /// The subsystem is broken. Restart it. Example: auth token expired.
    Fatal,
    /// The whole service is broken. Shut down. Example: database corruption.
    Shutdown,
}

fn classify_error(err: &anyhow::Error) -> ErrorSeverity {
    // Check for specific error types by downcasting
    if let Some(hermes_err) = err.downcast_ref::<HermesError>() {
        return match hermes_err {
            HermesError::Http(_) => ErrorSeverity::Transient,
            HermesError::Api(msg) if msg.contains("rate limit") => ErrorSeverity::Transient,
            HermesError::Api(msg) if msg.contains("401") => ErrorSeverity::Fatal,
            HermesError::Tool(_) => ErrorSeverity::Permanent,
            HermesError::Config(_) => ErrorSeverity::Shutdown,
            HermesError::ContextOverflow { .. } => ErrorSeverity::Permanent,
            _ => ErrorSeverity::Permanent,
        };
    }

    if let Some(reqwest_err) = err.downcast_ref::<reqwest::Error>() {
        if reqwest_err.is_timeout() || reqwest_err.is_connect() {
            return ErrorSeverity::Transient;
        }
    }

    ErrorSeverity::Permanent
}
```

### Retry with Exponential Backoff

For transient errors (network issues, rate limits), use exponential backoff with jitter.

```rust
use std::time::Duration;
use rand::Rng;

async fn retry_with_backoff<F, Fut, T>(
    name: &str,
    max_retries: u32,
    mut f: F,
) -> Result<T, anyhow::Error>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T, anyhow::Error>>,
{
    let mut attempt = 0;

    loop {
        match f().await {
            Ok(val) => return Ok(val),
            Err(e) => {
                attempt += 1;
                let severity = classify_error(&e);

                match severity {
                    ErrorSeverity::Transient if attempt <= max_retries => {
                        // Exponential backoff: 1s, 2s, 4s, 8s... capped at 60s
                        let base = Duration::from_secs(1 << (attempt - 1).min(6));
                        // Add jitter: 0-50% of base delay
                        let jitter_ms = rand::thread_rng().gen_range(0..=base.as_millis() / 2);
                        let delay = base + Duration::from_millis(jitter_ms as u64);

                        tracing::warn!(
                            task = name,
                            attempt,
                            max_retries,
                            delay_ms = delay.as_millis() as u64,
                            "Transient error, retrying: {}",
                            e
                        );
                        tokio::time::sleep(delay).await;
                    }
                    _ => return Err(e),
                }
            }
        }
    }
}
```

### Supervisor Pattern for Subsystem Restart

The supervisor pattern monitors child tasks and restarts them on failure, with circuit-breaking to prevent restart storms.

```rust
use std::time::{Duration, Instant};

struct SupervisorConfig {
    max_restarts: u32,
    restart_window: Duration,
    backoff_base: Duration,
}

impl Default for SupervisorConfig {
    fn default() -> Self {
        Self {
            max_restarts: 5,
            restart_window: Duration::from_secs(60),
            backoff_base: Duration::from_secs(1),
        }
    }
}

/// Supervise a subsystem, restarting on failure.
/// Returns only when max restarts are exhausted or cancellation is requested.
async fn supervise<F, Fut>(
    name: &str,
    token: CancellationToken,
    config: SupervisorConfig,
    mut factory: F,
) where
    F: FnMut(CancellationToken) -> Fut,
    Fut: std::future::Future<Output = Result<(), anyhow::Error>>,
{
    let mut restart_times: Vec<Instant> = Vec::new();
    let mut consecutive_failures: u32 = 0;

    loop {
        // Prune old restart times outside the window
        let cutoff = Instant::now() - config.restart_window;
        restart_times.retain(|t| *t > cutoff);

        // Circuit breaker: too many restarts in the window
        if restart_times.len() as u32 >= config.max_restarts {
            tracing::error!(
                subsystem = name,
                restarts = restart_times.len(),
                window_secs = config.restart_window.as_secs(),
                "Max restarts exceeded, giving up"
            );
            return;
        }

        let child_token = token.child_token();

        tracing::info!(subsystem = name, "Starting subsystem");

        tokio::select! {
            _ = token.cancelled() => {
                tracing::info!(subsystem = name, "Supervisor cancelled");
                return;
            }
            result = factory(child_token) => {
                match result {
                    Ok(()) => {
                        tracing::info!(subsystem = name, "Subsystem exited cleanly");
                        return;
                    }
                    Err(e) => {
                        consecutive_failures += 1;
                        restart_times.push(Instant::now());

                        let delay = config.backoff_base * consecutive_failures;
                        tracing::error!(
                            subsystem = name,
                            error = %e,
                            restart_in_ms = delay.as_millis() as u64,
                            "Subsystem failed, restarting"
                        );

                        tokio::time::sleep(delay).await;
                    }
                }
            }
        }
    }
}
```

**Usage:**

```rust
// In main service setup:
set.spawn(supervise(
    "telegram_poller",
    token.child_token(),
    SupervisorConfig::default(),
    |child_token| async move {
        telegram_poller(child_token, event_tx.clone()).await
    },
));
```

### Error Propagation: anyhow vs thiserror

The codebase already uses both correctly:
- `thiserror` for `HermesError` (typed, matchable domain errors in `ironhermes-core`)
- `anyhow` for application-level error propagation

**Recommendation:** Keep this dual approach. Add an `is_retryable()` method to `HermesError`:

```rust
impl HermesError {
    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            HermesError::Http(_)
                | HermesError::Api(_) // Could refine: only retryable API errors
                | HermesError::Io(_)
        )
    }

    pub fn is_fatal(&self) -> bool {
        matches!(
            self,
            HermesError::Config(_) | HermesError::Unauthorized(_)
        )
    }
}
```

---

## 4. Shared State Patterns

### The Decision Framework

| Pattern | Use When | Contention | Overhead |
|---------|----------|-----------|----------|
| `Arc<Mutex<T>>` | Short critical sections, infrequent access | Low-Medium | Lowest |
| `Arc<RwLock<T>>` | Read-heavy, rare writes | Low (reads) | Low |
| `Arc<DashMap<K,V>>` | High-concurrency maps, fine-grained locking | Low | Medium |
| Message passing (channels) | State owned by one task, others request via channel | None (no shared mem) | Higher per-op |

### SessionStore: Use Arc<RwLock<HashMap>>

The current `SessionStore` is a plain `HashMap` owned by `GatewayRunner`. In a concurrent service, multiple agent tasks need to read/write sessions simultaneously.

```rust
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Thread-safe session store.
/// Read-heavy: many agent tasks check for existing sessions.
/// Write-rare: sessions are created/destroyed infrequently.
#[derive(Clone)]
pub struct SessionStore {
    sessions: Arc<RwLock<HashMap<String, GatewaySession>>>,
}

impl SessionStore {
    pub fn new() -> Self {
        Self {
            sessions: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub async fn get_or_create(
        &self,
        key: &SessionKey,
        model: &str,
    ) -> GatewaySession {
        let string_key = key.to_string_key();

        // Fast path: read lock to check existence
        {
            let sessions = self.sessions.read().await;
            if let Some(session) = sessions.get(&string_key) {
                return session.clone();
            }
        }

        // Slow path: write lock to create
        let mut sessions = self.sessions.write().await;
        // Double-check after acquiring write lock (another task may have created it)
        sessions
            .entry(string_key)
            .or_insert_with(|| GatewaySession::new(key.clone(), model))
            .clone()
    }

    pub async fn update_session<F>(&self, key: &SessionKey, f: F) -> bool
    where
        F: FnOnce(&mut GatewaySession),
    {
        let mut sessions = self.sessions.write().await;
        if let Some(session) = sessions.get_mut(&key.to_string_key()) {
            f(session);
            true
        } else {
            false
        }
    }

    pub async fn remove(&self, key: &SessionKey) -> Option<GatewaySession> {
        self.sessions.write().await.remove(&key.to_string_key())
    }

    pub async fn count(&self) -> usize {
        self.sessions.read().await.len()
    }
}
```

**Why RwLock, not Mutex:** Session lookups (reads) happen on every incoming message. Session creation (writes) happens only for new conversations. RwLock allows concurrent reads.

**Why tokio::sync::RwLock, not std::sync::RwLock:** The tokio RwLock is async-aware. It will not block the executor thread. Use `std::sync::Mutex` ONLY if the critical section is trivially short and never awaits.

### ToolRegistry: Arc<ToolRegistry> (Immutable After Init)

The `ToolRegistry` is built once at startup and never modified. It is already `Arc`-wrapped in the codebase. This is correct -- no lock needed for read-only data.

```rust
// Current pattern -- already correct
let mut registry = ToolRegistry::new();
registry.register_defaults();
let registry = Arc::new(registry); // Immutable after this point

// Clone the Arc for each task that needs it
let agent = AgentLoop::new(client, registry.clone(), max_iters);
```

If hot-reloading tools becomes a requirement, switch to `Arc<RwLock<ToolRegistry>>` or use `arc_swap::ArcSwap<ToolRegistry>` for lock-free reads with occasional replacement:

```rust
use arc_swap::ArcSwap;

let registry = Arc::new(ArcSwap::new(Arc::new(build_registry())));

// Readers (zero-cost, no lock):
let current = registry.load();
current.dispatch("tool_name", args).await?;

// Writer (atomic swap, readers see old or new, never partial):
let new_registry = Arc::new(build_updated_registry());
registry.store(new_registry);
```

### LLM Client Config: Use watch Channel

For runtime-changeable config like model selection or temperature:

```rust
use tokio::sync::watch;

struct AgentConfig {
    model: String,
    max_iterations: usize,
    temperature: Option<f64>,
}

// At startup
let (config_tx, config_rx) = watch::channel(AgentConfig {
    model: "nous/hermes-3".into(),
    max_iterations: 25,
    temperature: None,
});

// Agent reads config at the start of each run (not mid-run)
async fn run_agent(config_rx: &watch::Receiver<AgentConfig>) {
    let config = config_rx.borrow().clone();
    // Use config for this entire agent run
}

// Admin command updates config
config_tx.send(AgentConfig { model: "new-model".into(), ..current })?;
```

---

## 5. Backpressure and Rate Limiting

### Bounded Channels as Natural Backpressure

The simplest and most effective backpressure mechanism: bounded `mpsc` channels. When the buffer is full, `send().await` blocks the producer until the consumer catches up.

```rust
// 32 pending messages before the Telegram poller blocks
let (tx, rx) = mpsc::channel::<IncomingMessage>(32);
```

Choose buffer size based on:
- How long an agent run takes (30-120s for complex runs)
- How many concurrent runs you support
- Acceptable queue depth before messages feel "delayed"

For IronHermes, `32-64` is reasonable. With 4 concurrent agent runs taking ~60s each, a buffer of 32 represents ~8 minutes of queued work, which is already too much. Consider a smaller buffer (8-16) with explicit overflow handling.

### Semaphore for Concurrent Agent Run Limiting

`tokio::sync::Semaphore` limits the number of concurrent agent runs. This is critical because each agent run consumes significant resources (LLM API calls, tool execution).

```rust
use tokio::sync::Semaphore;

const MAX_CONCURRENT_AGENTS: usize = 4;

struct AgentDispatcher {
    semaphore: Arc<Semaphore>,
    agent: Arc<AgentLoop>,
}

impl AgentDispatcher {
    fn new(agent: Arc<AgentLoop>) -> Self {
        Self {
            semaphore: Arc::new(Semaphore::new(MAX_CONCURRENT_AGENTS)),
            agent,
        }
    }

    async fn dispatch(&self, event: MessageEvent) -> Result<String, anyhow::Error> {
        // Acquire a permit -- blocks if all slots are taken
        let _permit = self.semaphore.acquire().await?;

        // The permit is held for the duration of the agent run
        let messages = vec![/* build from event */];
        let result = self.agent.run(messages).await?;

        Ok(result.final_response.unwrap_or_default())
        // _permit is dropped here, releasing the slot
    }

    /// Non-blocking: returns None if all slots are busy
    fn try_dispatch(&self, event: MessageEvent) -> Option<impl Future<Output = Result<String>>> {
        let permit = self.semaphore.clone().try_acquire_owned().ok()?;
        let agent = self.agent.clone();

        Some(async move {
            let messages = vec![/* build from event */];
            let result = agent.run(messages).await?;
            drop(permit);
            Ok(result.final_response.unwrap_or_default())
        })
    }
}
```

### Per-User Rate Limiting

Prevent a single user from monopolizing the service:

```rust
use std::collections::HashMap;
use std::time::Instant;
use tokio::sync::Mutex;

struct RateLimiter {
    limits: Mutex<HashMap<String, Vec<Instant>>>,
    max_requests: usize,
    window: Duration,
}

impl RateLimiter {
    fn new(max_requests: usize, window: Duration) -> Self {
        Self {
            limits: Mutex::new(HashMap::new()),
            max_requests,
            window,
        }
    }

    /// Returns Ok(()) if allowed, Err with wait duration if rate limited.
    async fn check(&self, user_id: &str) -> Result<(), Duration> {
        let mut limits = self.limits.lock().await;
        let now = Instant::now();
        let cutoff = now - self.window;

        let timestamps = limits.entry(user_id.to_string()).or_default();

        // Remove expired entries
        timestamps.retain(|t| *t > cutoff);

        if timestamps.len() >= self.max_requests {
            // How long until the oldest entry expires
            let wait = timestamps[0] + self.window - now;
            Err(wait)
        } else {
            timestamps.push(now);
            Ok(())
        }
    }
}

// Usage: 3 requests per 60 seconds per user
let limiter = Arc::new(RateLimiter::new(3, Duration::from_secs(60)));

// In the message handler:
match limiter.check(&event.sender_id).await {
    Ok(()) => { /* proceed with agent run */ }
    Err(wait) => {
        let msg = format!("Rate limited. Try again in {} seconds.", wait.as_secs());
        send_reply(event.chat_id, &msg).await;
    }
}
```

### Per-Chat Deduplication / Queuing

Prevent overlapping agent runs for the same chat (a common issue when users send multiple messages while the agent is still processing):

```rust
use std::collections::HashSet;
use tokio::sync::Mutex;

struct ChatGuard {
    active_chats: Mutex<HashSet<String>>,
}

impl ChatGuard {
    fn new() -> Self {
        Self {
            active_chats: Mutex::new(HashSet::new()),
        }
    }

    /// Try to acquire the chat lock. Returns false if an agent run is already active.
    async fn try_acquire(&self, chat_id: &str) -> bool {
        self.active_chats.lock().await.insert(chat_id.to_string())
    }

    async fn release(&self, chat_id: &str) {
        self.active_chats.lock().await.remove(chat_id);
    }
}

// Usage:
if !chat_guard.try_acquire(&event.chat_id).await {
    send_reply(&event.chat_id, "I'm still working on your previous request...").await;
    return;
}

// Run agent...

chat_guard.release(&event.chat_id).await;
```

---

## 6. Testing Async Code

### tokio::test Macro

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use tokio::time::{timeout, Duration};

    // Basic async test
    #[tokio::test]
    async fn test_session_store_concurrent_access() {
        let store = SessionStore::new();
        let key = SessionKey::new(Platform::Telegram, "chat_123");

        // Simulate concurrent access
        let store1 = store.clone();
        let key1 = key.clone();
        let handle1 = tokio::spawn(async move {
            store1.get_or_create(&key1, "model-a").await
        });

        let store2 = store.clone();
        let key2 = key.clone();
        let handle2 = tokio::spawn(async move {
            store2.get_or_create(&key2, "model-a").await
        });

        let (s1, s2) = tokio::join!(handle1, handle2);
        // Both should get the same session
        assert_eq!(
            s1.unwrap().session_id,
            s2.unwrap().session_id
        );
        assert_eq!(store.count().await, 1);
    }

    // Test with controlled time (no real waiting)
    #[tokio::test]
    async fn test_rate_limiter_window() {
        tokio::time::pause(); // Enable time manipulation

        let limiter = RateLimiter::new(2, Duration::from_secs(60));

        assert!(limiter.check("user1").await.is_ok());
        assert!(limiter.check("user1").await.is_ok());
        assert!(limiter.check("user1").await.is_err()); // Rate limited

        // Advance time past the window
        tokio::time::advance(Duration::from_secs(61)).await;

        assert!(limiter.check("user1").await.is_ok()); // Allowed again
    }
}
```

### Testing Channels and Pipelines

```rust
#[tokio::test]
async fn test_message_pipeline() {
    let (tx, mut rx) = mpsc::channel::<String>(10);

    // Simulate producer
    tokio::spawn(async move {
        tx.send("hello".into()).await.unwrap();
        tx.send("world".into()).await.unwrap();
        // tx dropped here, closing the channel
    });

    let mut messages = Vec::new();
    while let Some(msg) = rx.recv().await {
        messages.push(msg);
    }

    assert_eq!(messages, vec!["hello", "world"]);
}

#[tokio::test]
async fn test_cancellation_token_propagation() {
    let parent = CancellationToken::new();
    let child = parent.child_token();

    let handle = tokio::spawn({
        let child = child.clone();
        async move {
            child.cancelled().await;
            "cancelled"
        }
    });

    // Cancel the parent
    parent.cancel();

    // Child should also be cancelled
    let result = timeout(Duration::from_secs(1), handle).await;
    assert_eq!(result.unwrap().unwrap(), "cancelled");
}
```

### Mocking Async Traits

Use `mockall` for mocking async traits. Works with `async_trait`.

**Add dependency:** `mockall = "0.13"` (dev dependency)

```rust
use mockall::automock;

#[async_trait]
#[automock]
pub trait LlmProvider: Send + Sync {
    async fn chat_completion(
        &self,
        messages: &[ChatMessage],
    ) -> Result<ChatResponse, anyhow::Error>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_agent_loop_completes_naturally() {
        let mut mock_llm = MockLlmProvider::new();

        // Set up expectations
        mock_llm
            .expect_chat_completion()
            .times(1)
            .returning(|_| {
                Ok(ChatResponse {
                    choices: vec![Choice {
                        message: ChatMessage::assistant("Hello!"),
                        finish_reason: "stop".into(),
                    }],
                    usage: Some(Usage {
                        prompt_tokens: 10,
                        completion_tokens: 5,
                        total_tokens: 15,
                    }),
                })
            });

        let registry = Arc::new(ToolRegistry::new());
        let agent = AgentLoop::new(
            Box::new(mock_llm),
            registry,
            10,
        );

        let result = agent
            .run(vec![ChatMessage::user("Hi")])
            .await
            .unwrap();

        assert!(result.finished_naturally);
        assert_eq!(result.turns_used, 1);
        assert_eq!(result.final_response, Some("Hello!".into()));
    }

    #[tokio::test]
    async fn test_agent_loop_with_tool_calls() {
        let mut mock_llm = MockLlmProvider::new();
        let mut call_count = 0;

        mock_llm
            .expect_chat_completion()
            .times(2)
            .returning(move |_| {
                call_count += 1;
                if call_count == 1 {
                    // First call: request a tool
                    Ok(ChatResponse {
                        choices: vec![Choice {
                            message: ChatMessage::assistant_with_tool_calls(
                                vec![ToolCall {
                                    id: "tc_1".into(),
                                    function: FunctionCall {
                                        name: "read_file".into(),
                                        arguments: r#"{"path": "test.txt"}"#.into(),
                                    },
                                }],
                            ),
                            finish_reason: "tool_calls".into(),
                        }],
                        usage: None,
                    })
                } else {
                    // Second call: final answer
                    Ok(ChatResponse {
                        choices: vec![Choice {
                            message: ChatMessage::assistant("File contents: hello"),
                            finish_reason: "stop".into(),
                        }],
                        usage: None,
                    })
                }
            });

        // ... run and assert
    }
}
```

### Testing Timeouts and Edge Cases

```rust
#[tokio::test]
async fn test_agent_run_timeout() {
    // Agent run should not hang forever
    let result = timeout(
        Duration::from_secs(5),
        agent.run(messages),
    )
    .await;

    assert!(result.is_ok(), "Agent run timed out");
}

#[tokio::test]
async fn test_graceful_shutdown_under_load() {
    let token = CancellationToken::new();
    let (tx, mut rx) = mpsc::channel::<String>(100);

    // Spawn a worker that processes messages
    let worker_token = token.child_token();
    let handle = tokio::spawn(async move {
        let mut processed = 0;
        loop {
            tokio::select! {
                _ = worker_token.cancelled() => {
                    // Drain remaining messages
                    while rx.try_recv().is_ok() {
                        processed += 1;
                    }
                    return processed;
                }
                Some(_msg) = rx.recv() => {
                    processed += 1;
                    // Simulate work
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            }
        }
    });

    // Send some messages
    for i in 0..10 {
        tx.send(format!("msg_{}", i)).await.unwrap();
    }

    // Give worker time to process some
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Initiate shutdown
    token.cancel();

    let processed = handle.await.unwrap();
    assert!(processed > 0, "Worker should have processed some messages");
}
```

### Test Utilities

```rust
/// Test helper: create a mock message event
#[cfg(test)]
pub fn test_message_event(chat_id: &str, text: &str) -> MessageEvent {
    MessageEvent {
        platform: Platform::Telegram,
        message_id: uuid::Uuid::new_v4().to_string(),
        chat_id: chat_id.into(),
        sender_id: "test_user".into(),
        content: text.into(),
        attachments: Vec::new(),
        thread_id: None,
        chat_type: "dm".into(),
        chat_name: None,
        sender_name: Some("Test User".into()),
        replied_to_id: None,
    }
}

/// Test helper: create a channel pair with a pre-loaded message
#[cfg(test)]
pub async fn channel_with_message<T: Send + 'static>(
    msg: T,
) -> mpsc::Receiver<T> {
    let (tx, rx) = mpsc::channel(1);
    tx.send(msg).await.unwrap();
    drop(tx); // Close the channel so recv returns None after the message
    rx
}
```

---

## 7. Recommendations for IronHermes

### Priority 1: Replace AtomicBool with CancellationToken

**Where:** `TelegramAdapter` (and future adapters)
**Why:** The current `AtomicBool` + `handle.abort()` pattern does not allow graceful drain of in-flight agent runs. CancellationToken integrates with `select!`, supports hierarchical cancellation, and is the idiomatic tokio approach.
**Effort:** Low. Replace `Arc<AtomicBool>` with `CancellationToken`, update the poll loop to use `select!`.

### Priority 2: Add Semaphore-Based Concurrency Limiting

**Where:** `TelegramAdapter::start()` where it spawns per-message handlers
**Why:** Currently, every incoming Telegram message spawns an unbounded agent run. Under load (or spam), this can exhaust memory, overload the LLM API, and make the service unresponsive. A `Semaphore` with 4-8 permits caps concurrent agent runs.
**Effort:** Low. Wrap the spawn with `semaphore.acquire()`.

### Priority 3: Channel-Based Pipeline Architecture

**Where:** Refactor `TelegramAdapter` from monolithic to pipeline
**Why:** The current design mixes polling, dispatch, and response delivery in a single spawned task closure. Separating into channels enables:
- Independent error handling per stage
- Backpressure (bounded mpsc)
- Per-user rate limiting in the dispatch stage
- Testability (inject mock channels)

**Target architecture:**
```
TelegramPoller --mpsc--> AgentDispatcher --oneshot--> ResponseDeliverer
                              |
                         Semaphore(4)
                              |
                        JoinSet<AgentRun>
```
**Effort:** Medium. Significant refactor of the gateway crate.

### Priority 4: SessionStore Concurrency

**Where:** `SessionStore` in `session.rs`
**Why:** The current `HashMap`-based store cannot be safely shared across tasks. Wrap in `Arc<RwLock<HashMap>>` so multiple agent runs can access sessions concurrently.
**Effort:** Low. Change the struct internals, update the API to async.

### Priority 5: Supervisor Pattern for the Gateway

**Where:** `GatewayRunner::start()`
**Why:** If the Telegram poller crashes (auth error, network partition), the entire service dies. A supervisor that restarts the poller with backoff keeps the service alive.
**Effort:** Medium. Implement the `supervise()` function shown above, wrap each adapter in it.

### Priority 6: Error Classification

**Where:** `HermesError` in `error.rs`
**Why:** The service needs to distinguish between "retry this request" (network timeout), "skip this message" (malformed input), and "restart the subsystem" (auth failure). Adding `is_retryable()` and severity classification enables automated recovery.
**Effort:** Low. Add methods to the existing error enum.

### Additional Dependencies to Add

```toml
[workspace.dependencies]
# Add these:
tokio-util = { version = "0.7", features = ["rt"] }  # CancellationToken
mockall = "0.13"                                       # Test mocking
rand = "0.8"                                           # Jitter for backoff
# Optional, if hot-reload needed:
# arc-swap = "1"                                       # Lock-free Arc replacement
```

### Anti-Patterns to Avoid

1. **Do not use `std::sync::Mutex` with `.await` inside the critical section.** This blocks the tokio executor thread. Always use `tokio::sync::Mutex` if you await while holding the lock.

2. **Do not use `tokio::spawn` without tracking the JoinHandle.** Every spawned task should either be in a JoinSet or have its handle stored for shutdown coordination.

3. **Do not use `handle.abort()` as the primary shutdown mechanism.** Abort is a last resort. Use CancellationToken for cooperative shutdown, abort only after a timeout.

4. **Do not clone large data to avoid sharing.** The current `SessionStore` returns cloned sessions. For read-heavy access, prefer returning references via RwLock guards or using `Arc<Session>` internally.

5. **Do not use unbounded channels (`mpsc::unbounded_channel`).** They remove backpressure and can cause unbounded memory growth under load. Always use bounded channels and handle the pressure explicitly.

---

## Confidence Assessment

| Topic | Confidence | Notes |
|-------|-----------|-------|
| JoinSet API | HIGH | Stable since tokio 1.21, well-established |
| CancellationToken | HIGH | Standard pattern in tokio-util, widely adopted |
| Channel patterns | HIGH | Core tokio primitives, stable API |
| Supervisor pattern | MEDIUM | Custom implementation; no standard crate dominates |
| Semaphore rate limiting | HIGH | Built-in tokio primitive |
| mockall for async traits | MEDIUM | Works well but verify current version compatibility with edition 2024 |
| arc-swap | MEDIUM | Stable crate, but verify it is needed before adding |

## Sources

- tokio crate documentation (docs.rs/tokio)
- tokio-util crate documentation (docs.rs/tokio-util)
- Existing IronHermes codebase (analyzed directly)
- Training data for established Rust async patterns (may be up to 12 months stale)

**Note:** Web search and doc fetching were unavailable during this research session. All patterns are based on training data knowledge of tokio 1.x APIs and direct codebase analysis. Verify specific API signatures against `docs.rs/tokio/latest` before implementation, particularly for any tokio features stabilized after mid-2025.
