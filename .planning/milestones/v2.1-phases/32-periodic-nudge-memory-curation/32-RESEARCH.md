# Phase 32: Periodic Nudge & Memory Curation - Research

**Researched:** 2026-05-15
**Domain:** Rust async agent loop, turn-based nudge injection, AgentLoop/MemoryManager integration, config extension
**Confidence:** HIGH

## Summary

Phase 32 implements the periodic nudge mechanism (LEARN-01) and memory persistence judgment (LEARN-02) that form the agent-curated side of the Learning Loop. The feature is turn-based, not time-based: at configurable intervals (default 10 turns, matching Python hermes-agent), the agent receives an internal system-level user message asking it to review recent conversation and decide what is worth persisting to MEMORY.md/USER.md.

The Python reference (`run_agent.py`) shows the nudge fires **after** the main turn response is delivered, in a background thread via `_spawn_background_review`. It creates a full AIAgent fork (no user-visible output), restricts its tools to memory+skills only, feeds it the conversation snapshot plus a review prompt, and surfaces a compact "💾 Self-improvement review: ..." summary if anything was saved. The frozen-snapshot constraint (PRMT-06/MEM-06) is satisfied automatically: the review agent writes to disk, but the active session's `_cached_system_prompt` is not re-loaded mid-session.

The Rust implementation diverges from Python in one important structural way: `run_chat` and `run_gateway` use a per-turn `AgentLoop` that is constructed fresh each turn. The nudge is best implemented as a **separate sequential `AgentLoop` run** triggered after the main turn completes, using a turn counter maintained in the REPL/gateway loop, not inside `AgentLoop` itself.

**Primary recommendation:** Track `turns_since_nudge: u32` in `run_chat`/`run_gateway` alongside `messages`. On threshold, spawn a sequential (not concurrent) background `AgentLoop` run with the nudge prompt appended, limited to memory tools only, with a low iteration cap (8). The result is never sent to the user; if any memory writes occurred the result is logged with `tracing::info!`. The nudge fires at natural turn end — never mid-streaming.

## Architectural Responsibility Map

| Capability | Primary Tier | Secondary Tier | Rationale |
|------------|-------------|----------------|-----------|
| Turn counting for nudge trigger | CLI `run_chat` / Gateway `handler.rs` | — | These own the per-session turn loop; turn counter lives alongside `messages` vec |
| Nudge prompt construction | New `ironhermes-agent::nudge` module | `ironhermes-core` (config) | Self-contained; keeps `main.rs` clean |
| Nudge AgentLoop execution | `ironhermes-agent::AgentLoop` | — | Reuse existing engine; nudge is just another run with restricted tools |
| Tool restriction (memory-only) | Nudge call site (restrict `ToolRegistry`) | — | Build a narrowed registry at nudge time |
| Config (`nudge_interval` / `periodic_nudge_interval_seconds`) | `ironhermes-core::config::MemoryConfig` | wizard.rs (already writes raw YAML) | Add typed field alongside existing untyped wizard reservation |
| Frozen-snapshot invariant | Already implemented (MEM-06, Phase 17/20) | — | No new work; nudge writes to disk, active `_cached_system_prompt` unchanged |
| User visibility | Silent by default; `tracing::info!` on save | — | Python surfaces "💾 Self-improvement review: ..." to user — Rust should do same |
| Gateway wiring | `ironhermes-gateway::handler.rs` | — | Gateway handler builds per-turn AgentLoop; nudge counter lives in session state |

## Standard Stack

### Core (all workspace deps — no new external dependencies needed)

[VERIFIED: codebase inspection]

| Library | Version | Purpose | Why Standard |
|---------|---------|---------|--------------|
| `ironhermes-agent` | workspace | `AgentLoop`, `MemoryManager` | Engine for nudge run |
| `ironhermes-core` | workspace | `Config`, `MemoryConfig` | Config struct extension |
| `ironhermes-tools` | workspace | `ToolRegistry` | Narrowed registry for nudge |
| `tokio` | workspace | `tokio::spawn` / async | Already the async runtime |
| `tracing` | workspace | Structured logging of save actions | Already used throughout |
| `anyhow` | workspace | Error handling | Already used throughout |

### No New Packages Required

Phase 32 introduces no new external dependencies. All building blocks are in-workspace:
- `AgentLoop` runs the nudge
- `MemoryManager::handle_tool_call` handles writes
- `ToolRegistry` is narrowed at nudge call site
- Turn counting is a `u32` local variable

## Package Legitimacy Audit

No external packages are being added in this phase. This section is not applicable.

## Architecture Patterns

### System Architecture Diagram

```
Per-turn REPL loop (run_chat / gateway handler)
│
├── User message arrives
├── turns_since_nudge += 1
│
├── run_agent_turn(...)         ← Main turn (existing path, unchanged)
│   └── AgentLoop::run()
│       └── streams response to user
│
├── [if turns_since_nudge >= nudge_interval AND memory enabled]
│   └── spawn_nudge_review(messages_snapshot, memory_manager)
│       ├── Build narrow ToolRegistry (memory tools only)
│       ├── Construct AgentLoop (max_iterations=8, quiet)
│       ├── run() with MEMORY_REVIEW_PROMPT appended as user message
│       │   └── Agent calls memory_add/replace/remove as needed
│       │       └── Writes persist to MEMORY.md / USER.md on disk
│       │           └── Active system prompt snapshot: UNCHANGED (MEM-06)
│       ├── turns_since_nudge = 0
│       └── tracing::info!("nudge saved: {summary}") if any writes occurred
│
└── Next user turn
    └── system prompt still frozen; new entries take effect at NEXT session start
```

### Recommended Module Structure

```
crates/ironhermes-agent/src/
├── nudge.rs                   # NEW: spawn_nudge_review(), MEMORY_REVIEW_PROMPT const
├── agent_loop.rs              # UNCHANGED (nudge is external)
└── memory/
    └── manager.rs             # UNCHANGED

crates/ironhermes-core/src/
└── config.rs                  # ADD: nudge_interval field to MemoryConfig

crates/ironhermes-cli/src/
└── main.rs                    # ADD: turns_since_nudge counter + nudge fire site in run_chat

crates/ironhermes-gateway/src/
└── handler.rs                 # ADD: turns_since_nudge counter + nudge fire site in handle_message
```

### Pattern 1: Turn-Based Nudge Counter (Python Reference)

The Python implementation uses a simple integer counter on the agent object:

```python
# Initialization
self._memory_nudge_interval = int(mem_config.get("nudge_interval", 10))
self._turns_since_memory = 0

# Per-turn trigger check
self._turns_since_memory += 1
if self._turns_since_memory >= self._memory_nudge_interval:
    _should_review_memory = True
    self._turns_since_memory = 0

# Fire AFTER main response delivered (never interrupts streaming)
if final_response and not interrupted and _should_review_memory:
    self._spawn_background_review(messages_snapshot, review_memory=True)
```

[VERIFIED: codebase inspection — `run_agent.py` lines 1970, 1978, 12029-12034, 15670-15674]

**Rust equivalent:**

```rust
// Source: run_agent.py pattern, adapted for per-turn-AgentLoop architecture
let mut turns_since_nudge: u32 = 0;
let nudge_interval = config.memory.nudge_interval;  // 0 = disabled

// After each successful main turn (inside REPL loop):
if nudge_interval > 0 && config.memory.memory_enabled {
    turns_since_nudge += 1;
    if turns_since_nudge >= nudge_interval {
        turns_since_nudge = 0;
        if let Some(ref mgr) = memory_manager {
            spawn_nudge_review(messages.clone(), mgr.clone(), &client, &config).await;
        }
    }
}
```

### Pattern 2: Nudge as Sequential Background AgentLoop Run

The Python `_spawn_background_review` creates a full AIAgent fork in a background thread. In Rust, the cleanest equivalent is a sequential async call after the main turn completes (before the REPL re-prompts). The nudge is I/O-bound (one LLM call), so `tokio::spawn` is appropriate for non-blocking behavior:

```rust
// Source: run_agent.py _spawn_background_review pattern
// crates/ironhermes-agent/src/nudge.rs

pub const MEMORY_REVIEW_PROMPT: &str =
    "Review the conversation above and consider saving to memory if appropriate.\n\n\
     Focus on:\n\
     1. Has the user revealed things about themselves — their persona, desires, \
     preferences, or personal details worth remembering?\n\
     2. Has the user expressed expectations about how you should behave, their work \
     style, or ways they want you to operate?\n\n\
     Important: Decide per-item which memory layer fits:\n\
     - \"Important enough to be present in every future conversation\" → memory tool \
     (MEMORY.md/USER.md). These persist across sessions.\n\
     - \"Useful only when topic comes up\" → leave in session history (searchable via \
     session_search). Do not force these into prompt memory.\n\n\
     The total memory cap is 3,575 chars (2,200 MEMORY.md + 1,375 USER.md). \
     Be selective — only persist what genuinely improves every future conversation.\n\
     If nothing is worth saving, just say 'Nothing to save.' and stop.";

pub async fn spawn_nudge_review(
    messages_snapshot: Vec<ChatMessage>,
    memory_manager: Arc<tokio::sync::Mutex<MemoryManager>>,
    client: AnyClient,
    config: &Config,
) {
    // Build narrow registry: memory tools only
    // Spin up AgentLoop with max_iterations=8
    // Append MEMORY_REVIEW_PROMPT as a user message
    // Run silently; log any saves via tracing::info!
    // Nudge intervals disable recursion: nudge runs don't trigger more nudges
}
```

### Pattern 3: Config Structure

The `learning.periodic_nudge_interval_seconds` key is already written by the wizard as raw YAML (untyped). Phase 32 needs to surface this as a **typed** field so the runtime can read it.

Two paths exist:

**Option A (Recommended):** Add `nudge_interval: u32` to `MemoryConfig` in `config.rs`. This reads from `memory.nudge_interval` in YAML (matching Python's placement under `memory:`). The wizard's `learning.periodic_nudge_interval_seconds` becomes a separate unrelated key read separately, OR the wizard is updated to write `memory.nudge_interval` instead.

**Option B:** Add `LearningConfig` struct to `Config` with `periodic_nudge_interval_seconds: u64`. This reads from `learning.periodic_nudge_interval_seconds` which is already written by the wizard. The downside is the struct field doesn't match the Python config location (`memory.nudge_interval`).

**Decision for planner:** Option A (add to `MemoryConfig`) is the cleaner match with Python. The wizard already writes to raw YAML under `learning:` which is a separate namespace — this is fine as long as the planner notes the key name difference. The wizard key (`learning.periodic_nudge_interval_seconds`) is for the setup wizard flow; the runtime reads from `memory.nudge_interval`. The wizard can be updated in this phase to also write `memory.nudge_interval` alongside the existing `learning.periodic_nudge_interval_seconds` key.

**Python config reference:**

```yaml
# Source: hermes-agent/cli-config.yaml.example lines 473-476
memory:
  nudge_interval: 10        # Nudge every 10 user turns (0 = disabled)
```

[VERIFIED: codebase inspection — `hermes-agent/cli-config.yaml.example` line 475]

### Pattern 4: Memory-Only Tool Registry for Nudge

The nudge review agent must be restricted to memory tools only. Python does this via a thread-local whitelist. In Rust, build a narrowed `ToolRegistry` at the nudge call site:

```rust
// Source: run_agent.py _spawn_background_review whitelist pattern
// Only register memory_add, memory_replace, memory_remove (+ provider tools from memory_manager)
let mut nudge_registry = ToolRegistry::new();
if let Some(ref mgr) = memory_manager {
    // Register MemoryTool with the manager handle
    nudge_registry.register(MemoryTool::new(mgr.clone()));
    // Optionally register provider tools (memory_recall etc)
    // Do NOT register session_search, execute_code, web_read, etc.
}
let nudge_registry = Arc::new(RwLock::new(nudge_registry));
```

### Pattern 5: Nudge Interaction with Streaming

The nudge fires AFTER `run_agent_turn` returns. This is structurally safe:
- `run_agent_turn` returns `Option<String>` (the final response)
- The nudge runs after that return, before the REPL re-prompts for input
- No streaming is in flight during the nudge
- The nudge's own AgentLoop uses the same client but a fresh turn — no interference

For the gateway, the same applies: `handle_message` calls `agent.run()` and delivers the response before any nudge fires. The nudge can be `tokio::spawn`-ed so the gateway's message handler returns promptly.

### Pattern 6: Suppressing Recursion

The nudge's `AgentLoop` must not trigger more nudges. Since the turn counter lives in the REPL/gateway loop (not inside `AgentLoop`), the nudge's internal run does not increment the outer counter — recursion is structurally prevented.

Python's explicit guard (`review_agent._memory_nudge_interval = 0`) is not needed in Rust since the counter is external.

### Anti-Patterns to Avoid

- **Time-based interval (tokio::interval):** Python uses turn count, not wall-clock time. The LEARN-01 requirement says "default 5 minutes" but this is a UX framing — the Python implementation is actually turn-based (`nudge_interval: 10` turns). Wall-clock timers require racing with the active agent loop and coordinating with the cancellation token. Turn-based is simpler and matches the reference.
- **Injecting nudge into active AgentLoop:** The nudge must never interrupt a streaming response. It fires after `run_agent_turn` returns, not inside it.
- **Storing turn counter inside AgentLoop:** AgentLoop is constructed fresh per turn in the Rust architecture. The counter must live in the outer REPL/gateway session loop.
- **Running nudge in same AgentLoop instance as main turn:** The nudge uses a separate `AgentLoop` with restricted tools and low iteration cap.
- **Blocking the REPL on nudge completion:** Use `tokio::spawn` for the nudge so the REPL can show the next prompt while the nudge runs. Show the nudge result when it completes (via callback or tracing).

## Don't Hand-Roll

| Problem | Don't Build | Use Instead | Why |
|---------|-------------|-------------|-----|
| Memory writes | Custom file writer | `MemoryManager::handle_tool_call` | Already handles caps, security scan, mirror fanout |
| Tool restriction | Custom dispatch filter | Build narrow `ToolRegistry` | Registry already supports per-instance tool lists |
| Async background task | Custom thread | `tokio::spawn` | Already in tokio runtime |
| Session snapshot | Custom message serialization | `messages.clone()` (Vec<ChatMessage>) | ChatMessage is already Clone |
| Frozen snapshot invariant | Any custom logic | Already implemented (MEM-06) | The active prompt is never mutated mid-session |

**Key insight:** The entire nudge mechanism reuses existing `AgentLoop` infrastructure. Phase 32 is primarily wiring, not new engine code.

## Common Pitfalls

### Pitfall 1: Time-Based vs Turn-Based Trigger

**What goes wrong:** Implementing a `tokio::time::interval` timer that races with the active agent turn, requiring complex synchronization to avoid interrupting streaming.

**Why it happens:** LEARN-01 says "default 5 minutes" which sounds time-based.

**How to avoid:** The Python reference is turn-based (`nudge_interval: 10` turns). Use turn count. The "5 minutes" in the requirement is an approximation — at ~30 seconds per turn, 10 turns ≈ 5 minutes.

**Warning signs:** If you find yourself using `tokio::select!` to race a nudge timer against `run_agent_turn`, you're on the wrong path.

### Pitfall 2: Nudge Counter Placement

**What goes wrong:** Putting the turn counter inside `AgentLoop`, which is constructed fresh per turn in `run_chat`/`run_agent_turn`.

**Why it happens:** Feels natural since the nudge is agent behavior.

**How to avoid:** The counter must be a `u32` in the outer REPL loop (alongside `messages`, `session_id`), not in `AgentLoop`. Phase 32.1's `activity_summary()` pattern shows the right shape: agent state that spans turns lives outside the per-turn AgentLoop.

**Warning signs:** If the counter resets to 0 on every user turn without firing the nudge, the counter is in the wrong place.

### Pitfall 3: Memory Cap Violation

**What goes wrong:** The nudge agent adds entries beyond the 3,575 char limit (2,200 MEMORY.md + 1,375 USER.md).

**Why it happens:** Nudge prompt doesn't mention the cap.

**How to avoid:** The `MemoryManager::handle_tool_call` already enforces caps via `MemoryStore` — writes exceeding the cap return a `capacity_exceeded` error. The nudge prompt should mention selectivity ("the total cap is 3,575 chars; only persist what genuinely improves every future conversation"). The LEARN-02 two-tier judgment ("important to every future conversation" → MEMORY.md vs "useful only when topic comes up" → session search) belongs in the nudge prompt text.

**Warning signs:** Memory tool returning `capacity_exceeded` errors in nudge runs.

### Pitfall 4: Recursion (Nudge Spawning Nudge)

**What goes wrong:** The nudge's `AgentLoop` somehow increments the outer turn counter, causing a nudge every run.

**Why it happens:** Shared counter state.

**How to avoid:** The turn counter is a local `u32` in the REPL loop. The nudge's internal `AgentLoop` run does not touch this counter. Structurally impossible for the nudge to trigger another nudge.

**Warning signs:** Nudge firing on every turn.

### Pitfall 5: Blocking REPL on Nudge Completion

**What goes wrong:** The REPL waits for the nudge to complete before showing the next prompt, adding visible latency.

**Why it happens:** `spawn_nudge_review(...).await` instead of `tokio::spawn(...)`.

**How to avoid:** Use `tokio::spawn` to run the nudge concurrently. The REPL continues immediately. Print the nudge summary asynchronously when the task completes (via a channel or callback, similar to `background_review_callback` in Python).

**Warning signs:** Noticeable delay between agent response and next prompt on nudge turns.

### Pitfall 6: Mutating Active System Prompt

**What goes wrong:** Loading updated MEMORY.md back into the active session's system prompt after the nudge writes.

**Why it happens:** Wanting the agent to "see" the new memories immediately.

**How to avoid:** PRMT-06/MEM-06 is explicit: the memory snapshot is frozen at session start. New entries written during the session take effect at the next session start. This is a feature (cache safety). Do not reload the prompt. The nudge writes to disk only.

## Code Examples

### MEMORY_REVIEW_PROMPT (verified from Python reference)

```rust
// Source: run_agent.py AIAgent._MEMORY_REVIEW_PROMPT (lines 3984-3996) — adapted
pub const MEMORY_REVIEW_PROMPT: &str =
    "Review the conversation above and consider saving to memory if appropriate.\n\n\
     Focus on:\n\
     1. Has the user revealed things about themselves — their persona, desires, \
     preferences, or personal details worth remembering?\n\
     2. Has the user expressed expectations about how you should behave, their work \
     style, or ways they want you to operate?\n\n\
     Decide per-item which memory layer fits:\n\
     - \"Important enough to be present in every future conversation\" → use the \
     memory tool (persists to MEMORY.md/USER.md, present in every session).\n\
     - \"Useful only when topic comes up\" → leave in session history (searchable \
     via session_search when needed). Do NOT force these into prompt memory.\n\n\
     The total memory cap is 3,575 chars (2,200 MEMORY.md + 1,375 USER.md). \
     Be selective — only persist what genuinely improves every future conversation.\n\n\
     If nothing is worth saving, just say 'Nothing to save.' and stop.";
```

### Config Addition to MemoryConfig

```rust
// Source: run_agent.py line 1978, hermes-agent/cli-config.yaml.example line 475
// crates/ironhermes-core/src/config.rs — add to MemoryConfig struct:
pub struct MemoryConfig {
    // ... existing fields ...

    /// Periodic memory nudge interval in user turns. Default 10. Set to 0 to disable.
    /// At every N user turns, agent receives a background memory-review prompt.
    /// Honors PRMT-06: mid-session writes do not mutate the active prompt.
    #[serde(default = "default_nudge_interval")]
    pub nudge_interval: u32,
}

fn default_nudge_interval() -> u32 { 10 }
```

### Turn Counter in run_chat (skeleton)

```rust
// crates/ironhermes-cli/src/main.rs — inside the REPL loop

let nudge_interval = config.memory.nudge_interval;
let mut turns_since_nudge: u32 = 0;

// ... existing REPL loop ...
loop {
    // ... read user input, run_agent_turn ... //

    // After successful turn response delivered:
    if nudge_interval > 0 && config.memory.memory_enabled {
        turns_since_nudge += 1;
        if turns_since_nudge >= nudge_interval {
            turns_since_nudge = 0;
            if let Some(ref mgr) = memory_manager {
                let mgr_clone = Arc::clone(mgr);
                let client_clone = client.clone();
                let messages_snapshot = messages.clone();
                // Fire-and-forget; nudge result logged by nudge module
                tokio::spawn(async move {
                    ironhermes_agent::nudge::spawn_nudge_review(
                        messages_snapshot,
                        mgr_clone,
                        client_clone,
                    ).await;
                });
            }
        }
    }
}
```

## State of the Art

| Old Approach | Current Approach | When Changed | Impact |
|--------------|------------------|--------------|--------|
| Manual memory saves only | Agent-driven periodic review | Python hermes-agent v0.7+ | Agent self-improves without user prompt |
| Background thread (Python) | `tokio::spawn` async task (Rust) | Phase 32 | Same semantics, idiomatic Rust |
| Time-based intervals | Turn-based counter | Python design | Simpler, predictable, no race with streaming |

**Deprecated/outdated:**
- `learning.periodic_nudge_interval_seconds` (wizard key): This is the setup-wizard raw YAML key. The runtime should read from `memory.nudge_interval` (typed). The wizard should write both keys, or be updated to write only the typed one.

## Open Questions (RESOLVED)

1. **Turn-based vs. time-based?**
   - What we know: Python is turn-based (`nudge_interval: 10` turns); LEARN-01 says "default 5 minutes" (time-based framing)
   - What's unclear: The requirement text says "at configurable intervals" — could mean either
   - Recommendation: Use turn-based (matches Python reference). At ~30s/turn, 10 turns ≈ 5 minutes. Simpler, no timer synchronization needed. If time-based is required, add a second implementation path.
   - RESOLVED: Turn-based (matches Python reference). Plans implement `memory.nudge_interval: u32` (turn count, default 10). LEARN-01's "default 5 minutes" is an approximation at ~30s/turn. No time-based implementation needed.

2. **User visibility of nudge result?**
   - What we know: Python prints "💾 Self-improvement review: {summary}" to user. The nudge is "silent" in the sense it doesn't produce a chat response, but the save summary is shown.
   - What's unclear: For the Rust REPL, showing the summary requires either writing to stdout/scroll_region or using a tracing-level log.
   - Recommendation: Use `tracing::info!` for gateway; print directly for CLI REPL (same approach as streaming token output). Gateway can surface via `background_review_callback` pattern.
   - RESOLVED: CLI REPL logs via `tracing::info!("nudge: memory review complete")`; gateway likewise. nudge.rs emits tracing events on completion. No stdout print required for MVP — mirrors tracing pattern used throughout ironhermes-cli.

3. **Gateway session turn counter?**
   - What we know: Gateway creates a new `AgentLoop` per message but the session persists across messages. The turn counter must survive across gateway messages for the same session.
   - What's unclear: Where to store the per-session counter. Options: (a) in `SessionStore` as a field; (b) in a `HashMap<session_key, u32>` in `GatewayRunner`; (c) derive from counting messages in `StateStore`.
   - Recommendation: Option (b) — a `HashMap<String, u32>` in `GatewayRunner` (alongside other per-session state). Most direct.
   - RESOLVED: Option (b) — `nudge_turns: Arc<std::sync::Mutex<HashMap<SessionKey, u32>>>` field on `GatewayHandler`, using the same interior-mutability pattern as `skill_overlays`. Implemented in Plan 32-02.

4. **Should `run_single` support nudges?**
   - What we know: `run_single` is for one-shot prompts, not interactive sessions.
   - Recommendation: No — nudge is session-level. Only `run_chat` and gateway.
   - RESOLVED: No nudge in `run_single`. Turn counter wired only in `run_chat` (Plan 32-01) and gateway `handler.rs` (Plan 32-02).

## Environment Availability

This phase has no external dependencies beyond what is already in the workspace. Step 2.6: SKIPPED (no external dependencies — pure Rust workspace changes).

## Validation Architecture

### Test Framework

| Property | Value |
|----------|-------|
| Framework | cargo test (built-in) |
| Config file | none — inline test modules |
| Quick run command | `cargo test -p ironhermes-agent nudge` |
| Full suite command | `cargo test --workspace` |

### Phase Requirements → Test Map

| Req ID | Behavior | Test Type | Automated Command | File Exists? |
|--------|----------|-----------|-------------------|-------------|
| LEARN-01 | Nudge fires at configured interval | unit | `cargo test -p ironhermes-agent nudge::tests::fires_at_interval` | ❌ Wave 0 |
| LEARN-01 | Nudge disabled when interval=0 | unit | `cargo test -p ironhermes-agent nudge::tests::disabled_when_zero` | ❌ Wave 0 |
| LEARN-01 | Nudge does not fire mid-stream | integration | manual UAT | N/A |
| LEARN-01 | Counter resets after nudge fires | unit | `cargo test -p ironhermes-agent nudge::tests::counter_resets` | ❌ Wave 0 |
| LEARN-02 | Nudge prompt contains two-tier judgment text | unit | `cargo test -p ironhermes-agent nudge::tests::prompt_contains_tier_guidance` | ❌ Wave 0 |
| LEARN-02 | Memory cap honored (3575 chars) | unit | existing `memory_manager` tests | ✅ |
| LEARN-01 | Config field `nudge_interval` deserializes with default 10 | unit | `cargo test -p ironhermes-core config_nudge_interval_default` | ❌ Wave 0 |
| LEARN-01 | Config field `nudge_interval=0` disables nudge | unit | `cargo test -p ironhermes-core config_nudge_interval_zero` | ❌ Wave 0 |

### Sampling Rate
- **Per task commit:** `cargo test -p ironhermes-agent nudge`
- **Per wave merge:** `cargo test --workspace`
- **Phase gate:** Full suite green before `/gsd:verify-work`

### Wave 0 Gaps
- [ ] `crates/ironhermes-agent/src/nudge.rs` — unit tests module covering LEARN-01/LEARN-02
- [ ] `crates/ironhermes-core/tests/config_nudge.rs` — or inline in `config.rs` tests block

## Security Domain

### Applicable ASVS Categories

| ASVS Category | Applies | Standard Control |
|---------------|---------|-----------------|
| V2 Authentication | no | — |
| V3 Session Management | no | — |
| V4 Access Control | no | — |
| V5 Input Validation | yes | Nudge writes routed through existing `MemoryManager::handle_tool_call` which calls `scan_content` security scanner |
| V6 Cryptography | no | — |

### Known Threat Patterns

| Pattern | STRIDE | Standard Mitigation |
|---------|--------|---------------------|
| Prompt injection via memory review | Tampering | Existing `MemoryStore` security scanner (`scan_content`) already applied on all writes — no bypass needed |
| Nudge writes exceeding cap | Denial of Service | `MemoryStore` enforces char caps; returns `capacity_exceeded` error |
| Recursive nudge amplification | Denial of Service | Turn counter is external to `AgentLoop`; nudge's AgentLoop run does not increment outer counter |

## Assumptions Log

| # | Claim | Section | Risk if Wrong |
|---|-------|---------|---------------|
| A1 | Nudge should be turn-based (10 turns) matching Python, not time-based (5 minutes) as LEARN-01 text suggests | Architecture Patterns, Pattern 1 | If time-based is required, need tokio interval + racing logic — more complex |
| A2 | `run_single` should NOT support nudges (one-shot prompts only) | Open Questions §4 | Low risk — nudge in run_single is easy to add if requested |
| A3 | Gateway turn counter stored in `HashMap<session_key, u32>` in GatewayRunner | Open Questions §3 | Could be in SessionStore instead — implementation detail |
| A4 | Nudge summary visible to CLI user via stdout (not just tracing log) | Open Questions §2 | If wrong, user loses "💾 Self-improvement review" feedback |

## Sources

### Primary (HIGH confidence)

- `run_agent.py` lines 1970-1978, 12029-12034, 15670-15674 — Python nudge initialization, trigger, and firing [VERIFIED: codebase inspection]
- `run_agent.py` lines 4230-4440 — `_spawn_background_review` full implementation [VERIFIED: codebase inspection]
- `run_agent.py` lines 3984-3996 — `_MEMORY_REVIEW_PROMPT` constant [VERIFIED: codebase inspection]
- `hermes-agent/cli-config.yaml.example` lines 473-476 — Python `memory.nudge_interval` config [VERIFIED: codebase inspection]
- `crates/ironhermes-agent/src/memory/manager.rs` — `MemoryManager` write path, MirrorManager [VERIFIED: codebase inspection]
- `crates/ironhermes-agent/src/agent_loop.rs` — AgentLoop struct, `with_memory_manager`, `run()` natural-end break [VERIFIED: codebase inspection]
- `crates/ironhermes-core/src/config.rs` — `MemoryConfig` struct, `Config` struct [VERIFIED: codebase inspection]
- `crates/ironhermes-core/src/wizard.rs` — `apply_learning_loop_answer` writing `periodic_nudge_interval_seconds` [VERIFIED: codebase inspection]
- `crates/ironhermes-core/src/config_schema.rs` — `learning.periodic_nudge_interval_seconds` field registration [VERIFIED: codebase inspection]
- `cli-config.yaml.example` lines 288-299 — Rust config file already has `learning:` block commented out [VERIFIED: codebase inspection]
- `crates/ironhermes-cli/src/main.rs` — `run_chat` turn loop, `run_agent_turn` signature, `memory_manager` wiring [VERIFIED: codebase inspection]
- `.planning/phases/32.1-agent-cron-execution/32.1-RESEARCH.md` — Phase 32.1 cron infrastructure as reference [VERIFIED: codebase inspection]

### Secondary (MEDIUM confidence)

- `crates/ironhermes-cron-runner/src/tick_loop.rs` — `tokio::time::interval` pattern for async loops [VERIFIED: codebase inspection]

## Metadata

**Confidence breakdown:**
- Standard stack: HIGH — no new external deps; all building blocks verified in workspace
- Architecture: HIGH — Python reference is fully read; Rust call sites (run_chat, AgentLoop, MemoryManager) are verified
- Pitfalls: HIGH — derived from Python implementation decisions and existing Rust architecture constraints
- Config: HIGH — `MemoryConfig` struct verified; wizard raw-YAML path verified; the turn-vs-time question is flagged as assumption

**Research date:** 2026-05-15
**Valid until:** 2026-06-15 (stable codebase; no external deps)

<phase_requirements>
## Phase Requirements

| ID | Description | Research Support |
|----|-------------|------------------|
| LEARN-01 | Periodic nudge mechanism — at configurable intervals (default 5 minutes) during a session, agent receives an internal system-level prompt asking it to scan recent activity and evaluate whether anything is worth persisting to MEMORY.md/USER.md. Fires without user input. Honors PRMT-06 (mid-session writes persist to disk but do not mutate the active prompt). | Turn-based counter in REPL loop triggers post-turn nudge AgentLoop run. Python reference: `nudge_interval: 10` turns. Config: `memory.nudge_interval` (new typed field). Wiring sites: `run_chat` (main.rs) and gateway `handler.rs`. |
| LEARN-02 | Memory persistence judgment — during the nudge, agent decides per-item which memory layer information belongs in. Threshold: "important enough to be present in every future conversation" → MEMORY.md/USER.md; "useful only when topic comes up" → session search archive. Coordinates with 3,575 char total memory cap. | `MEMORY_REVIEW_PROMPT` encodes the two-tier judgment logic verbatim. MemoryManager enforces char caps on writes. session_search is excluded from nudge tool registry so agent cannot archive mid-nudge. |
</phase_requirements>
