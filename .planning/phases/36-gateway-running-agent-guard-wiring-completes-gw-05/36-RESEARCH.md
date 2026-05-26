# Phase 36: Gateway running-agent guard wiring (completes GW-05) — Research

**Researched:** 2026-05-24
**Domain:** Rust async concurrency / gateway slash-command dispatch / per-session state management
**Confidence:** HIGH

---

<user_constraints>
## User Constraints (from CONTEXT.md)

### Locked Decisions

- **D-01 — Bypass list:** `/stop`, `/new`, `/status`, `/queue`. `/approve` and `/deny` are NOT on the bypass list (TODO stubs with no approval queue). Add them when real approval queue lands.
- **D-02 — Non-bypass behavior:** Reject with error message `Agent is running. Use /stop to interrupt or /queue to send after this turn.` No queueing, no replay. Delivered via `with_rate_limit_retry(|| adapter.send_message(...))`.
- **D-03 — State model:** Single `AtomicBool` per session, stored as `HashMap<SessionKey, Arc<AtomicBool>>` (or analogous) on `SessionStore`. Wraps the existing `Arc<RwLock<SessionStore>>` at the handler.
- **D-04 — `/model` mid-turn:** Rejected. Same path as D-02. Closes codex HIGH-2 TOCTOU finding.
- **D-05 — State ownership:** State lives in `SessionStore` (gateway-local), NOT in `AgentRuntime`. Keeps `AgentRuntime` channel-agnostic.
- **D-06 — Set/clear discipline:** `RunningAgentGuard` RAII type. `Drop` clears the flag. Constructed at top of `run_agent`, dropped at function exit. Covers success, error, and `?` early-return exits.

### Claude's Discretion

- Exact storage shape: new `HashMap<SessionKey, Arc<AtomicBool>>` field on `SessionStore` vs. new `running: Arc<AtomicBool>` field on `GatewaySession`. Planner picks.
- Bypass-list check: inline `match cmd.name { "stop" | "new" | "status" | "queue" => true, _ => false }` vs. `fn is_bypass(name: &str) -> bool` in `ironhermes-core::commands`. Planner picks.
- Where to place `RunningAgentGuard`: inside `handler.rs` (private) vs. sibling module.
- Exact error message wording within D-02 intent.
- Whether to delete vs. rewrite the stale comment at `handler.rs:377-380`.

### Deferred Ideas (OUT OF SCOPE)

- Per-turn LLM cancellation (`handler.rs:1032`)
- CLI + gateway unified running-agent mechanism
- Queue-and-replay UX (hermes-agent pattern)
- `/approve`/`/deny` bypass list addition
- `/model` defer-to-next-turn
- `enum AgentState { Idle, Running, Cancelling, Queued }` richer state model
</user_constraints>

<phase_requirements>
## Phase Requirements

| ID | Description | Research Support |
|----|-------------|------------------|
| GW-05 | Gateway slash command dispatch with running-agent guard | Guard logic gap confirmed at `handler.rs:377-380`; fix path identified (add per-session `Arc<AtomicBool>` to `SessionStore`, populate in `handle_slash_command`, set/clear in `run_agent` via RAII guard) |
</phase_requirements>

---

## Summary

Phase 36 closes GW-05, a known partial-shipment from Phase 21.1. The gateway's `handle_slash_command` function already performs full `CommandRouter::resolve()` + `handlers::dispatch()` dispatch. What is missing is exactly one mechanism: a per-session boolean that `handle_slash_command` can check before dispatching to decide whether the session has an agent turn in-flight. As of today, `handler.rs:377-380` hardcodes `Arc::new(AtomicBool::new(false))` with a comment that explicitly defers the real wiring. This phase replaces that shim.

The architecture decision is well-settled by the CONTEXT.md decisions: per-session `Arc<AtomicBool>` stored in `SessionStore`, populated into `CommandContext.agent_running` (already correctly typed since Phase 21.1), checked in `handle_slash_command` after `resolve()` and before `dispatch()`, set/cleared around `run_agent` via a `RunningAgentGuard` RAII wrapper. Total surface area is two files in the gateway crate (`handler.rs`, `session.rs`) plus tests. No changes to `ironhermes-core`, `ironhermes-agent`, CLI, TUI, or web UI.

The research additionally confirmed the full cross-interface picture per the expanded mandate: CLI REPL is correctly wired; TUI/ratatui is correctly wired; web UI has NO slash-command interception at all (all messages go directly to `run_turn`), which is a broader architectural gap that is explicitly out of Phase 36 scope per D-05. The non-slash message path in the gateway's `MessageHandler::handle` is the one item needing a planner decision: it routes non-slash input directly to `run_agent` without a guard check, so a user can bypass the guard by omitting `/`.

**Primary recommendation:** Add a `running: Arc<AtomicBool>` field to `GatewaySession` (Option B — cleaner lifecycle match). Retrieve the per-session handle in `handle_slash_command`, apply the guard check, and wrap the `run_agent` body in `RunningAgentGuard::new(flag)`. Also guard the non-slash path in `MessageHandler::handle` with the same flag (reject with the same D-02 error message). This is a ~150 line net change across two files plus tests.

---

## Architectural Responsibility Map

| Capability | Primary Tier | Secondary Tier | Rationale |
|------------|-------------|----------------|-----------|
| Per-session running-agent state | Gateway / SessionStore | — | One `AgentRuntime` per process serves many sessions; gateway-local `SessionStore` is the natural per-session keyed store |
| Guard policy enforcement (bypass / reject) | Gateway / handle_slash_command | — | Guard must fire at the dispatch boundary, before `handlers::dispatch()`. That call site is inside the gateway. |
| `CommandContext.agent_running` population | Gateway / handle_slash_command | — | Gateway constructs `CommandContext` here; it must supply the real per-session handle, not a fresh false flag |
| Set/clear around `run_agent` | Gateway / run_agent | — | Single `run_agent` function entry point covers all call sites in `handler.rs` (lines 523, 589, 772, 797, 1244) |
| `cmd_stop` reading `agent_running` | ironhermes-core / handlers | — | Already reads `ctx.agent_running.load()`; no change needed. After Phase 36 this becomes meaningful on gateway. |
| CommandRouter resolution | ironhermes-core / CommandRouter | — | Unchanged by this phase. Guard check uses the resolved `CommandDef.name`. |
| Non-slash message guard | Gateway / MessageHandler::handle | — | The non-slash routing path must also be gated; same flag, same D-02 error. |

---

## Standard Stack

No external packages are installed by this phase. All changes are pure Rust using types already in the dependency tree.

### Core (all already in Cargo.toml)

| Library | Usage | Role in this phase |
|---------|-------|--------------------|
| `std::sync::atomic::AtomicBool` | `Arc<AtomicBool>` per session | The running-agent flag |
| `std::sync::atomic::Ordering::SeqCst` | `.store()` / `.load()` | Ensures visibility across async executor threads |
| `std::collections::HashMap` | `HashMap<SessionKey, Arc<AtomicBool>>` | Per-session state map if using Option A |
| `std::sync::Arc` | `Arc<AtomicBool>` | Shared ownership between `CommandContext` and `RunningAgentGuard` |

`[VERIFIED: codebase grep]` — all four types confirmed present in existing gateway code (`handler.rs:377`, `session.rs`, existing `Arc<std::sync::Mutex<HashMap<...>>>` patterns in `GatewayMessageHandler`).

### Alternatives Considered

| Instead of | Could Use | Tradeoff |
|------------|-----------|----------|
| `AtomicBool` per session | `RwLock<bool>` per session | `AtomicBool` is lock-free, cheaper. The flag is single-writer per session; no fairness concern. AtomicBool is correct. |
| Field on `GatewaySession` (Option B) | Separate `HashMap<SessionKey, Arc<AtomicBool>>` on `SessionStore` (Option A) | Option B is cleaner: lifecycle matches session automatically, no extra HashMap to manage. Option A is more explicit. Planner picks. |
| `RunningAgentGuard` RAII | Manual set/clear at each call site | Rust has no `finally`. Manual clear misses `?` early returns. RAII is the only correct idiom here (D-06). |
| Inline `matches!` for bypass check | `fn is_bypass(name: &str) -> bool` in `ironhermes-core::commands` | Shared function is better if/when CLI unification happens; inline is simpler for now. Claude's Discretion. |

---

## Package Legitimacy Audit

Not applicable — this phase installs no external packages.

---

## Architecture Patterns

### System Architecture Diagram

```
Incoming message (Telegram / Discord / etc.)
         |
         v
 MessageHandler::handle()
         |
         +--- message starts with '/' -----------------------+
         |                                                   |
         |                                           handle_slash_command()
         |                                                   |
         |                                       router.resolve(input)
         |                                                   |
         |                                 +-----------------+-----------------+
         |                                 |                                   |
         |                           Exact/PrefixMatch(def)                NotFound
         |                                 |                                   |
         |                    [NEW] get per-session flag                        |
         |                                 |                               run_agent()
         |                    +------------+------------+                (also guarded)
         |                    |                         |
         |              flag == true               flag == false
         |                    |                         |
         |          is_bypass(def.name)?        handlers::dispatch()
         |             |           |
         |           true        false
         |             |           |
         |   handlers::dispatch()  send_error(D-02 msg)
         |
         +--- message does NOT start with '/' ---------------+
                                                             |
                                                  [NEW] get per-session flag
                                                             |
                                              +-------------+-----------+
                                              |                         |
                                        flag == true             flag == false
                                              |                         |
                                   send_error(D-02 msg)           run_agent()
                                                                        |
                                                   [NEW] RunningAgentGuard::new(flag)
                                                         sets flag=true on construction
                                                                        |
                                                        runtime.run_turn(request).await
                                                                        |
                                                        [on Drop] flag=false (RAII)
```

### Recommended Project Structure

Changes are entirely within the existing structure:

```
crates/ironhermes-gateway/src/
  handler.rs      # PRIMARY CHANGE: guard check in handle_slash_command,
                  #   guard check in MessageHandler::handle,
                  #   RunningAgentGuard struct,
                  #   RunningAgentGuard::new() wrapping run_agent body
  session.rs      # PRIMARY CHANGE: add running: Arc<AtomicBool> to GatewaySession
                  #   (Option B) or running_agents HashMap to SessionStore (Option A)

crates/ironhermes-gateway/tests/
  running_agent_guard_tests.rs    # NEW: unit/integration tests for guard behavior
```

No new crates. No new modules required (RunningAgentGuard can live in `handler.rs` as a private type, or be extracted to a `guard.rs` sibling — planner picks).

### Pattern 1: RunningAgentGuard RAII

**What:** RAII wrapper that atomically sets the per-session flag on construction and clears it on drop.
**When to use:** Constructed at the top of `run_agent`, held for the entire function body. The `?` operator propagates errors through early returns; `Drop` fires unconditionally on any exit path.

```rust
// Source: CONTEXT.md specifics + standard Rust RAII idiom [VERIFIED: codebase]
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

struct RunningAgentGuard(Arc<AtomicBool>);

impl RunningAgentGuard {
    fn new(flag: Arc<AtomicBool>) -> Self {
        flag.store(true, Ordering::SeqCst);
        Self(flag)
    }
}

impl Drop for RunningAgentGuard {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

// Usage inside run_agent (receives Arc<AtomicBool> as parameter):
async fn run_agent(&self, session_key: &SessionKey, running_flag: Arc<AtomicBool>, ...) -> Result<...> {
    let _guard = RunningAgentGuard::new(running_flag);  // sets true, clears on any exit
    // ... existing run_agent body unchanged ...
}
```

**Async safety note:** `Drop` is synchronous. `AtomicBool::store` is synchronous. The guard can be safely held across `.await` points because Rust guarantees `Drop` runs when the owning scope exits, whether by normal completion, `?` early return, or future cancellation. `[ASSUMED]` — well-established Rust behavior confirmed in training data; low risk if wrong.

### Pattern 2: Guard check after resolve, before dispatch

**What:** After `router.resolve()` returns a resolved `CommandDef`, check the per-session flag. If running and not bypass, reject. If not running (or bypass), dispatch normally.
**When to use:** Insert at the exact point in `handle_slash_command` between the `resolve()` call and the `handlers::dispatch()` call.

```rust
// Inside handle_slash_command, replacing lines 377-380 region
// Source: CONTEXT.md specifics [VERIFIED: codebase grep of handler.rs:363-410]

// Retrieve (or create) the per-session running flag
let agent_running: Arc<AtomicBool> = {
    let store = self.session_store.read().await;
    store
        .sessions
        .get(&session_key.to_string_key())
        .map(|s| s.running.clone())
        .unwrap_or_else(|| Arc::new(AtomicBool::new(false)))
};

// Build CommandContext with the real flag (replaces the hardcoded-false shim)
let ctx = CommandContext::new(platform.clone(), session_key.to_string_key(), agent_running.clone());

// ... router.resolve(input) call (already exists) ...

// Guard check: after resolve(), before dispatch()
if agent_running.load(Ordering::SeqCst) {
    if !is_bypass(&resolved_def.name) {
        with_rate_limit_retry(|| adapter.send_message(
            chat_id,
            "Agent is running. Use /stop to interrupt or /queue to send after this turn.",
        )).await?;
        return Ok(());
    }
    // bypass command: fall through to dispatch
}

handlers::dispatch(&router, &ctx, resolved_def).await
```

**Important:** The check uses `resolved_def.name` (canonical name after alias resolution), not the raw user input string. `/reset` (alias for `new`) correctly bypasses the guard because it resolves to `"new"`. `[VERIFIED: codebase]` — confirmed in `registry.rs` that bypass-list command names match canonical registrations.

### Pattern 3: Per-session flag storage (Option B — recommended)

**What:** Add `running: Arc<AtomicBool>` field to `GatewaySession`. Lifecycle matches session automatically.

```rust
// session.rs:41 — current struct + new field
// Source: [VERIFIED: codebase — GatewaySession at session.rs:41]
pub struct GatewaySession {
    pub key: SessionKey,
    pub session_id: String,
    pub messages: Vec<Message>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub model: Option<String>,
    pub running: Arc<AtomicBool>,   // NEW — initialized false at session creation
}
```

Initialization site (wherever `GatewaySession` is constructed):
```rust
GatewaySession {
    // ... existing fields ...
    running: Arc::new(AtomicBool::new(false)),
}
```

### Pattern 4: is_bypass predicate

```rust
// Source: D-01 — bypass list
// Note: match on canonical name (post-resolve), not raw user input
fn is_bypass(name: &str) -> bool {
    matches!(name, "stop" | "new" | "status" | "queue")
    // TODO: add "approve" | "deny" when approval queue is implemented (D-01)
}
```

### Anti-Patterns to Avoid

- **Creating a new `AtomicBool` each call** (the current bug at `handler.rs:377-380`): the flag is always false and discarded after each call. The same `Arc` instance must be shared between the guard setter and the checker.
- **Global single `AtomicBool`:** One flag for the entire gateway conflates sessions. A turn in session A must not block session B. Codex HIGH-2 called this out explicitly.
- **Manual set/clear without RAII:** Any path that manually sets true/false will eventually miss a `?` branch. Rust has no `finally`. Use `RunningAgentGuard`.
- **Checking the flag before `resolve()`:** The guard check must happen after resolution so that alias resolution (e.g. `/reset` → `new`) is already applied.
- **Storing state in `AgentRuntime`:** D-05 is locked. `AgentRuntime` is channel-agnostic; state goes in `SessionStore`.
- **Only guarding the slash-command path:** The non-slash path in `MessageHandler::handle` also reaches `run_agent`. Both paths need the guard.

---

## Don't Hand-Roll

| Problem | Don't Build | Use Instead | Why |
|---------|-------------|-------------|-----|
| RAII set/clear | Manual try/finally or cleanup flags | `RunningAgentGuard` (8-line struct) | Rust has no `finally`; `?` early returns skip manual cleanup; RAII is the only correct approach |
| Per-session keying | Custom session-ID hashmap wrangling | New field on existing `GatewaySession` (Option B) | `SessionStore` already manages session lifecycle; piggyback on it |
| Cross-thread atomic | `Mutex<bool>` | `Arc<AtomicBool>` | AtomicBool is lock-free, cheaper, and sufficient for a single-writer single-reader flag |
| Alias-aware bypass check | Raw string comparison on user input | Use `resolved_def.name` after `router.resolve()` | Aliases already resolved by `CommandRouter`; checking raw input would miss `/reset` -> `new` |
| Queue-and-replay | Per-session pending message buffer | Not needed (D-02 is reject) | hermes-agent's queue approach requires significant infrastructure; Phase 36 intentionally uses reject |

**Key insight:** This phase is deliberately minimal. The full "queue-and-replay" pattern from hermes-agent is deferred (D-02 is reject, not queue). Do not import complexity from the reference implementation.

---

## Runtime State Inventory

Not applicable — this is not a rename/refactor/migration phase. Phase 36 adds a new behavioral guard to an existing dispatch path. No stored data, live service config, OS-registered state, secrets, or build artifacts are affected.

---

## Common Pitfalls

### Pitfall 1: Guard only covers slash commands, not free-text input

**What goes wrong:** After the fix, a user bypasses the guard by typing a non-`/` message during an active agent turn. `MessageHandler::handle` routes non-slash input directly to `run_agent` without checking the flag.
**Why it happens:** The guard check is inserted into `handle_slash_command`. The non-slash path in `MessageHandler::handle` (line ~1240) calls `run_agent` unconditionally.
**How to avoid:** Add an identical flag check in `MessageHandler::handle` before calling `run_agent` for non-slash messages. Reject with the same D-02 error. This is correct: there is no input queue (D-02), so a free-text message during an active turn should also be rejected.
**Warning signs:** Tests only cover slash-command guard scenarios; no test sends plain text to a Running session.

### Pitfall 2: Guard placed at call sites rather than inside `run_agent`

**What goes wrong:** `RunningAgentGuard` placed at one call site leaves other call sites unguarded. `handler.rs` has five call sites for `run_agent` (lines 523, 589, 772, 797, 1244).
**Why it happens:** The guard "feels" like it belongs at the call site.
**How to avoid:** Place `RunningAgentGuard::new(flag)` at the TOP of the `run_agent` function body. Pass the `Arc<AtomicBool>` as a parameter to `run_agent`. One guard, all five call sites covered.
**Warning signs:** Tests pass for one guarded call site; production traffic through the other four does not set the flag.

### Pitfall 3: Wrong atomic ordering on flag check

**What goes wrong:** Using `Ordering::Relaxed` on `load()` may read a stale value (flag appears false when it's actually true) on weakly-ordered architectures.
**Why it happens:** Relaxed ordering is the default in many examples; SeqCst feels "overly strict."
**How to avoid:** Use `Ordering::SeqCst` for both `store()` in `RunningAgentGuard` and `load()` in the guard check. Performance cost is negligible at per-message rates.
**Warning signs:** Occasional missed guard that is hard to reproduce, disappears under debug builds.

### Pitfall 4: Bypass list checked against raw user input

**What goes wrong:** `is_bypass("/stop")` includes the slash prefix; registered command names are `"stop"` without slash. Or `/reset` (alias for `new`) is not in the bypass set because raw input `"reset"` is checked instead of canonical `"new"`.
**Why it happens:** It feels natural to check what the user typed.
**How to avoid:** Check `resolved_def.name` (post-alias-resolution canonical name). Bypass list is `{"stop", "new", "status", "queue"}` matching canonical names.
**Warning signs:** `/stop` or `/reset` are rejected during an active turn instead of bypassing.

### Pitfall 5: Flag not cleared on agent error

**What goes wrong:** If `run_agent` returns `Err(...)`, manual cleanup at a specific branch might be missed. Session is permanently stuck in "Running" state; subsequent commands are all rejected.
**Why it happens:** Manual cleanup is error-prone under `?`.
**How to avoid:** `RunningAgentGuard`'s `Drop` impl handles this. As long as the guard is a local variable in `run_agent`, Rust guarantees `drop()` fires on every exit path including `?`.
**Warning signs:** Session permanently stuck in "Running" after any agent error.

### Pitfall 6: Option A (separate HashMap) creates a memory leak

**What goes wrong:** If `running_agents: HashMap<SessionKey, Arc<AtomicBool>>` is added to `SessionStore`, entries must be removed when sessions are cleaned up. Forgetting this causes unbounded HashMap growth.
**Why it happens:** Session cleanup is in one place; `running_agents` cleanup is in a separate place.
**How to avoid:** Prefer Option B (field on `GatewaySession`) which co-locates the flag with the session automatically. If Option A is chosen, ensure the session cleanup path also removes from `running_agents`.
**Warning signs:** Memory use grows unboundedly on long-running gateway with high session churn.

### Pitfall 7: `approve` / `deny` added to bypass list prematurely

**What goes wrong:** Adding `/approve` and `/deny` to the bypass list before the approval queue is implemented gives users the false impression these commands work during an active turn.
**Why it happens:** hermes-agent's bypass list includes them; cargo-culting it.
**How to avoid:** D-01 is explicit: do NOT include `/approve`/`/deny` until the approval queue is implemented. Add a `// TODO: add "approve" | "deny" when approval queue implemented` comment in `is_bypass`.
**Warning signs:** Bypass list in code has more than four entries.

---

## Code Examples

### The bug (current code to replace)

```rust
// handler.rs:377-380 — CURRENT incorrect implementation
// Source: [VERIFIED: codebase direct read]
// Build CommandContext (agent_running always false for gateway slash commands —
// the running-agent guard is a future enhancement using per-session state).
let agent_running = Arc::new(std::sync::atomic::AtomicBool::new(false));
let ctx = CommandContext::new(platform.clone(), session_key.to_string_key(), agent_running);
```

### GatewaySession with new field (Option B — recommended)

```rust
// session.rs — add to GatewaySession struct
// Source: [VERIFIED: codebase — GatewaySession at session.rs:41]
pub struct GatewaySession {
    pub key: SessionKey,
    pub session_id: String,
    pub messages: Vec<Message>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub model: Option<String>,
    pub running: Arc<AtomicBool>,  // NEW
}

// At session construction site:
// running: Arc::new(AtomicBool::new(false)),
```

### RunningAgentGuard (complete implementation)

```rust
// Placement: top of handler.rs or a sibling guard.rs module (planner picks)
// Source: D-06 from CONTEXT.md
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

struct RunningAgentGuard(Arc<AtomicBool>);

impl RunningAgentGuard {
    fn new(flag: Arc<AtomicBool>) -> Self {
        flag.store(true, Ordering::SeqCst);
        Self(flag)
    }
}

impl Drop for RunningAgentGuard {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}
```

### run_agent with guard

```rust
// handler.rs:801 — run_agent signature update (approximate — planner verifies exact sig)
// Source: D-06 from CONTEXT.md + codebase reading of run_agent at handler.rs:801
async fn run_agent(
    &self,
    // ... existing parameters ...
    running_flag: Arc<AtomicBool>,  // NEW — passed in from call site
) -> Result<...> {
    let _guard = RunningAgentGuard::new(running_flag);  // sets true; Drop clears false
    // ... rest of existing run_agent body unchanged ...
}
```

### is_bypass predicate

```rust
// Source: D-01 from CONTEXT.md
fn is_bypass(name: &str) -> bool {
    matches!(name, "stop" | "new" | "status" | "queue")
    // TODO(D-01): add "approve" | "deny" when approval queue is implemented
}
```

### Non-slash message guard in MessageHandler::handle

```rust
// handler.rs — MessageHandler::handle, before the run_agent call for non-slash messages
// Source: Pitfall 1 analysis above
let agent_running = {
    let store = self.session_store.read().await;
    store
        .sessions
        .get(&session_key.to_string_key())
        .map(|s| s.running.clone())
        .unwrap_or_else(|| Arc::new(AtomicBool::new(false)))
};

if agent_running.load(Ordering::SeqCst) {
    with_rate_limit_retry(|| adapter.send_message(
        chat_id,
        "Agent is running. Use /stop to interrupt or /queue to send after this turn.",
    )).await?;
    return Ok(());
}

// ... existing run_agent call ...
```

---

## Cross-Interface Wiring Survey (Expanded Mandate)

This section documents running-agent wiring state across all four interfaces, confirming D-05 and scoping Phase 36 correctly.

| Interface | Uses CommandRouter | CommandContext built | agent_running correctly set/cleared | Uses run_turn | Phase 36 changes |
|-----------|-------------------|---------------------|-------------------------------------|--------------|-----------------|
| Gateway | Yes — `handle_slash_command` calls `router.resolve()` then `handlers::dispatch()` | Yes — `CommandContext::new(platform, key, agent_running)` at `handler.rs:379-380` | **NO** — hardcoded `false` at `handler.rs:377-380` | Yes — `runtime.run_turn(request).await` in `run_agent` | **YES — this phase** |
| CLI REPL | Yes — REPL intercepts `/` before AgentLoop | Yes — built per-REPL-session | **YES** — `agent_running.store(true)` at `main.rs:1707`, `.store(false)` at `main.rs:2024` | Yes | None needed |
| TUI / ratatui | Yes — `tui_rata/commands.rs` resolves via CommandRouter | Yes | **YES** — derived from `app.pending_rx.is_some()` at `commands.rs:537` | Yes | None needed |
| Web UI | Instance held in `AppState.command_router` but used ONLY for `list_slash_commands()` REST endpoint | **NO** — `run_web_turn()` at `state.rs:208` goes directly to `runtime.run_turn()` without slash interception | **NO** — no `agent_running` state in web UI | Yes | Out of scope — separate phase |

`[VERIFIED: codebase]` — all four interfaces confirmed via direct file reads.

**Key finding:** The web UI gap (no slash interception whatsoever in the turn path) is a SEPARATE and LARGER gap than GW-05. Phase 36 is correctly scoped to the gateway only. Do not address web UI in this phase.

**State ownership validation (D-05):** `AgentRuntime` struct (`agent_runtime.rs:117`) has no per-session running state and no session identity. One `AgentRuntime` serves all sessions. Adding running-agent state to it would require per-session tracking inside a channel-agnostic struct — wrong abstraction. `SessionStore` is the correct owner. `[VERIFIED: codebase]`

---

## State of the Art

| Old Approach | Current Approach | Impact |
|--------------|-----------------|--------|
| Global `running` flag | Per-session `Arc<AtomicBool>` keyed by `SessionKey` | Correct isolation; codex HIGH-2 TOCTOU concern addressed |
| Manual set/clear with `try/finally` (Python hermes-agent) | Rust RAII `RunningAgentGuard` | Eliminates missed-cleanup class of bugs |
| Queue-and-replay during active turn (hermes-agent) | Reject with error message (D-02) | Simpler to implement correctly; queue UX deferred |
| `agent_running: Arc::new(AtomicBool::new(false))` shim | `session.running.clone()` from per-session state | Closes GW-05; makes `cmd_stop`'s `load()` call meaningful on gateway |

**Deprecated/outdated:**
- `Arc::new(AtomicBool::new(false))` shim at `handler.rs:377-380`: replaced entirely by this phase.
- The comment block at `handler.rs:377-380` describing this as "a future enhancement": stale after this phase ships; delete or rewrite.

---

## Assumptions Log

| # | Claim | Section | Risk if Wrong |
|---|-------|---------|---------------|
| A1 | Rust async `Drop` fires on every exit including future cancellation and `?`, making RAII safe across `.await` | Pattern 1 | Low — well-established Rust behavior. If wrong, flag might not clear on cancellation. |
| A2 | `run_agent` call sites in `handler.rs` are exactly lines 523, 589, 772, 797, 1244 (five total) | Common Pitfalls / Pitfall 2 | If there are additional call sites not found by visual inspection, they would be unguarded. Planner MUST grep `self.run_agent(` in `handler.rs` to verify complete count. |
| A3 | `SessionStore` is held behind `Arc<RwLock<SessionStore>>` at the handler level (allowing concurrent reads) | Architecture Patterns — Pattern 3 | If lock type is `Mutex`, the `read().await` pattern differs. `[ASSUMED]` — inferred from `Arc<std::sync::Mutex<HashMap<...>>>` patterns in `GatewayMessageHandler` but `SessionStore` wrapper not line-verified. |
| A4 | `GatewaySession` is constructed at a single, identifiable site in the codebase | Pattern 3 | If construction is spread across many sites, each must add the `running` field initialization. Planner must grep for `GatewaySession {` to find all construction sites. |

---

## Open Questions (RESOLVED)

All three questions were resolved by the planner during Phase 36 planning (2026-05-24). See `36-01-PLAN.md`, `36-02-PLAN.md`, and `36-03-PLAN.md`.

1. **Non-slash message guard (Pitfall 1)** — **RESOLVED**
   - What we know: `MessageHandler::handle` routes non-slash messages directly to `run_agent`. The slash-command guard does not cover this path. D-02 says "reject with error" for non-bypass commands during active turn.
   - What's unclear: Is a free-text message during an active turn "non-bypass" and therefore rejected? Or silently dropped?
   - Recommendation: Reject with the same D-02 error. There is no input queue; a free-text message is a request to run the agent, so rejecting it is consistent with the policy. Planner should make this explicit.
   - **RESOLVED IN PLAN:** `36-02-PLAN.md` Task 2 sub-edit F guards BOTH `MessageHandler::handle` AND `handle_with_multimodal` non-slash arms with the D-02 reject; `test_freetext_rejected_when_running` enforces.

2. **`run_agent` signature change — how does it receive the session's flag?** — **RESOLVED**
   - What we know: `run_agent` at `handler.rs:801` must receive the `Arc<AtomicBool>` to construct the guard inside it.
   - What's unclear: Does `run_agent` currently accept `SessionKey` directly? What parameters does it take?
   - Recommendation: Pass the `Arc<AtomicBool>` directly to `run_agent` as a new parameter. The call site already has the handle from `session.running.clone()`.
   - **RESOLVED IN PLAN:** `36-02-PLAN.md` Task 2 sub-edit C chose a slightly different but valid approach — `run_agent` fetches its own flag via `self.session_store.read().await.get_running_flag(&session_key)` at the top of its body, avoiding signature ripple across the 5 call sites. Plan-checker validated this as acceptable planner discretion (RESEARCH recommendation was non-locked).

3. **Option A vs Option B storage shape (Claude's Discretion)** — **RESOLVED**
   - Recommendation: Option B (field on `GatewaySession`). Lifecycle matches automatically; no separate HashMap to manage; no cleanup gap risk. Planner should document the rationale.
   - **RESOLVED IN PLAN:** `36-02-PLAN.md` Task 1 adds `pub running: Arc<AtomicBool>` directly to the `GatewaySession` struct (Option B). Rationale captured in plan notes — lifecycle matches automatically, no separate map to manage, no cleanup gap.

---

## Validation Architecture

### Test Framework

| Property | Value |
|----------|-------|
| Framework | Rust `#[cfg(test)]` modules + `cargo test` |
| Config file | Cargo.toml (workspace) |
| Quick run command | `cargo test -p ironhermes-gateway running_agent` |
| Full suite command | `cargo test -p ironhermes-gateway` |

### Phase Requirements → Test Map

| Req ID | Behavior | Test Type | Automated Command | File Exists? |
|--------|----------|-----------|-------------------|-------------|
| GW-05-1 | Per-session isolation: session A Running does not block session B | Unit | `cargo test -p ironhermes-gateway running_agent_guard::test_session_isolation` | Wave 0 |
| GW-05-2 | Guard rejects `/model` during active turn (returns D-02 error message) | Unit | `cargo test -p ironhermes-gateway running_agent_guard::test_model_rejected_when_running` | Wave 0 |
| GW-05-3 | Bypass: `/stop` dispatches even when flag is true | Unit | `cargo test -p ironhermes-gateway running_agent_guard::test_stop_bypasses_guard` | Wave 0 |
| GW-05-4 | Bypass: `/new` dispatches even when flag is true | Unit | `cargo test -p ironhermes-gateway running_agent_guard::test_new_bypasses_guard` | Wave 0 |
| GW-05-5 | Bypass: `/status` dispatches even when flag is true | Unit | `cargo test -p ironhermes-gateway running_agent_guard::test_status_bypasses_guard` | Wave 0 |
| GW-05-6 | Bypass: `/queue` dispatches even when flag is true | Unit | `cargo test -p ironhermes-gateway running_agent_guard::test_queue_bypasses_guard` | Wave 0 |
| GW-05-7 | Flag clears on `run_agent` returning `Ok(...)` | Unit | `cargo test -p ironhermes-gateway running_agent_guard::test_guard_clears_on_success` | Wave 0 |
| GW-05-8 | Flag clears on `run_agent` returning `Err(...)` | Unit | `cargo test -p ironhermes-gateway running_agent_guard::test_guard_clears_on_error` | Wave 0 |
| GW-05-9 | Alias `/reset` (resolves to `new`) bypasses guard | Unit | `cargo test -p ironhermes-gateway running_agent_guard::test_alias_bypasses_guard` | Wave 0 |
| GW-05-10 | Non-slash free-text during active turn is rejected (Pitfall 1) | Unit | `cargo test -p ironhermes-gateway running_agent_guard::test_freetext_rejected_when_running` | Wave 0 |
| GW-05-11 | `cmd_stop` reads a non-false `agent_running` on gateway (integration) | Integration | `cargo test -p ironhermes-gateway running_agent_guard::test_stop_reads_real_flag` | Wave 0 |

### Sampling Rate

- **Per task commit:** `cargo test -p ironhermes-gateway running_agent`
- **Per wave merge:** `cargo test -p ironhermes-gateway`
- **Phase gate:** Full gateway test suite green before `/gsd:verify-work`

### Wave 0 Gaps

- `crates/ironhermes-gateway/tests/running_agent_guard_tests.rs` — all 11 tests above (Wave 0 creates this file)
- Tests require a mock or stub for `SessionStore` that returns controllable `Arc<AtomicBool>` values

---

## Security Domain

### Applicable ASVS Categories

| ASVS Category | Applies | Standard Control |
|---------------|---------|-----------------|
| V2 Authentication | No | — |
| V3 Session Management | Yes | Per-session state keyed by `SessionKey`; no cross-session bleed |
| V4 Access Control | Yes | Guard policy: bypass list restricts which commands execute during active turn |
| V5 Input Validation | Yes | Bypass check on `resolved_def.name` (canonical, post-resolution) not raw user input |
| V6 Cryptography | No | — |

### Known Threat Patterns for this stack

| Pattern | STRIDE | Standard Mitigation |
|---------|--------|---------------------|
| TOCTOU on global flag (codex HIGH-2) | Tampering | Per-session `Arc<AtomicBool>` — each session has isolated state; no race between sessions |
| Model swap during in-flight API call (codex HIGH-2) | Tampering | D-04: `/model` during active turn is rejected; no credentials-swap-during-turn window |
| Bypassing guard via alias resolution | Elevation of Privilege | Check `resolved_def.name` post-resolution so all aliases of non-bypass commands are also rejected |
| Bypassing guard via non-slash input | Elevation of Privilege | Guard also applied in `MessageHandler::handle` non-slash path (Pitfall 1 fix) |
| Session isolation failure | Information Disclosure | Per-session `Arc<AtomicBool>` keyed by `SessionKey { platform, chat_id, user_id }`; one flag per session |

---

## Sources

### Primary (HIGH confidence)

- `[VERIFIED: codebase]` — `crates/ironhermes-gateway/src/handler.rs` — direct read; lines 62, 377-380, 801, 1032, 1220-1246 confirmed
- `[VERIFIED: codebase]` — `crates/ironhermes-gateway/src/session.rs` — direct read; `GatewaySession` at line 41, `SessionStore` at line 87, `SessionKey` at line 11
- `[VERIFIED: codebase]` — `crates/ironhermes-core/src/commands/context.rs:368` — `agent_running: Arc<AtomicBool>` field confirmed
- `[VERIFIED: codebase]` — `crates/ironhermes-core/src/commands/handlers.rs:127` — `cmd_stop` reads `ctx.agent_running.load(Ordering::SeqCst)`
- `[VERIFIED: codebase]` — `crates/ironhermes-core/src/commands/handlers.rs:1507` — `cmd_queue` TODO stub confirmed
- `[VERIFIED: codebase]` — `crates/ironhermes-agent/src/agent_runtime.rs:117` — `AgentRuntime` struct has no per-session running state; D-05 justified
- `[VERIFIED: codebase]` — `crates/iron_hermes_ui/src/server/state.rs:208` — `run_web_turn` goes directly to `runtime.run_turn()` without slash interception
- `[VERIFIED: codebase]` — `crates/iron_hermes_ui/src/server/ws.rs` — WebSocket handler confirms no slash interception before `run_web_turn`
- `[VERIFIED: codebase]` — `crates/ironhermes-cli/src/main.rs:1707,2024` — CLI `agent_running.store(true/false)` confirmed correctly wired
- `[VERIFIED: codebase]` — `crates/ironhermes-cli/src/tui_rata/commands.rs:537` — TUI `agent_running` from `app.pending_rx.is_some()` confirmed
- `.planning/phases/36-gateway-running-agent-guard-wiring-completes-gw-05/36-CONTEXT.md` — locked decisions D-01 through D-06
- `.planning/phases/21.1-slash-commands/21.1-REVIEWS.md` — codex HIGH-1 and HIGH-2 findings (origin of this phase)

### Secondary (MEDIUM confidence)

- `[VERIFIED: codebase]` — `crates/ironhermes-core/src/commands/registry.rs` — bypass-list command names confirmed matching canonical registrations (`stop`, `new`, `status`, `queue`, `approve`, `deny` all registered)

### Tertiary (LOW confidence)

- `[ASSUMED]` — Rust async Drop behavior fires on every exit path including `?` and future cancellation — training data knowledge, not fetched from Context7

---

## Metadata

**Confidence breakdown:**
- Standard stack: HIGH — no new packages; all types already in codebase
- Architecture: HIGH — direct codebase reads confirm all structural claims
- Pitfalls: HIGH — derived from codex review findings + direct code inspection
- Cross-interface survey: HIGH — all four interfaces read directly

**Research date:** 2026-05-24
**Valid until:** 2026-06-24 (stable codebase; no fast-moving external dependencies)
