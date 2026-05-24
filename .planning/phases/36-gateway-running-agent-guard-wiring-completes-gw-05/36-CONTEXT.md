# Phase 36: Gateway running-agent guard wiring — completes GW-05 — Context

**Gathered:** 2026-05-24
**Status:** Ready for planning
**Source:** /gsd-discuss-phase

<domain>
## Phase Boundary

Wire per-session running-agent state on the gateway so the existing `CommandRouter` dispatch path (`crates/ironhermes-gateway/src/handler.rs:handle_slash_command`) knows whether each session has an in-flight agent turn and can enforce a guard policy: a small bypass list of session-control commands always dispatches; everything else (including `/model`) is rejected with a clear error while the agent is running. Completes GW-05, which Phase 21.1 shipped only partially (dispatch via `resolve_command()` works; the guard never fires because `agent_running` is hardcoded `false` at `handler.rs:377-380`).

**In scope (Phase 36):**
- Add per-session running-agent state to gateway (storage in `SessionStore`, keyed by `SessionKey`)
- Populate `CommandContext.agent_running` with the per-session state inside `handle_slash_command`
- Set the per-session flag `true` immediately before `run_agent` and `false` after it returns (success, error, or cancel)
- Implement the guard policy in `handle_slash_command`: when the session's flag is `true`, only dispatch if the resolved `CommandDef.name` is in the bypass list; otherwise return an error response
- Tests: per-session isolation (one session Running, another Idle); guard rejects `/model` mid-turn; bypass list lets `/stop`, `/new`, `/status`, `/queue` through; flag clears on agent error and on cancellation paths
- Update Phase 21.1 threat model: T-21.1-05 was a discretionary guardrail; this phase makes it load-bearing

**Out of scope (will not change this phase):**
- Per-turn LLM cancellation — `handler.rs:1032` ("gateway has no per-turn cancel today") is a separate concern. `/stop` continues to target the subagent `process_registry` only, as it does today
- CLI parity (CLI already has its own `agent_running` plumbing in `main.rs` set around `run_agent_turn`; unifying CLI + gateway under one mechanism is deferred)
- Real implementations of `/queue`, `/approve`, `/deny` — they remain wired/stubbed exactly as today. `/queue` stays as the TODO-message handler at `handlers.rs:1507`; it just bypasses the guard so the user can run it during a turn (it'll still say "Agent loop not configured")
- AgentRuntime ownership of running state — state lives in `SessionStore` (gateway-local) per D-03 below. Pushing it into `AgentRuntime` would broaden blast radius beyond the gateway

</domain>

<decisions>
## Implementation Decisions

### Bypass list

- **D-01:** Bypass list = `/stop`, `/new`, `/status`, `/queue` — full hermes-agent parity with `gateway/run.py:1735-1852`. `/queue` is partially wired in IronHermes (returns a TODO-message at `handlers.rs:1507`), but listing it preserves parity so when real input-queue infrastructure lands (separate phase), no change to the guard is needed. `/approve` and `/deny` are NOT on the bypass list — they're TODO stubs with no approval queue, and including them would suggest functionality that doesn't exist. They can be added when the approval queue is implemented.

### Non-bypass behavior

- **D-02:** When a non-bypass command arrives during an active agent turn, reject with the error message: `Agent is running. Use /stop to interrupt or /queue to send after this turn.` Delivered via `with_rate_limit_retry(|| adapter.send_message(...))` like other gateway responses. No queueing, no replay, no per-turn cancel. Mirrors today's CLI behavior and matches the "reject with explanatory error" UX. A future phase can upgrade to queue-and-replay when a real per-session pending-message buffer is built.

### State model

- **D-03:** Per-session running-agent state model = a single `AtomicBool` (or equivalent) per session, stored as `HashMap<SessionKey, Arc<AtomicBool>>` (or analogous structure) on `SessionStore` (`crates/ironhermes-gateway/src/session.rs:87`). `SessionStore` is already wrapped in `Arc<RwLock<SessionStore>>` at the handler so concurrent reads + serialised writes are free. The `Arc<AtomicBool>` exists per-session so `CommandContext.agent_running` (already typed `Arc<AtomicBool>` from Phase 21.1, see `crates/ironhermes-core/src/commands/context.rs:368`) can hold the same handle the gateway flips around `run_agent`. No richer state machine yet — `/stop` only differentiates Running vs Idle today; upgrading to `enum {Idle, Running, Cancelling, ...}` waits until a per-turn cancel mechanism creates an actual observer for the third state.

### `/model` mid-turn

- **D-04:** `/model` during an active agent turn is rejected with an error response. Same path as D-02 (non-bypass → reject). Closes the codex HIGH-2 TOCTOU finding (credentials swapped while an API call is in flight). No special-casing for same-provider model changes; if the user wants to swap models, they `/stop` first. Future phase can add "defer to next turn" if UX feedback demands it.

### State ownership location (implicit from D-03)

- **D-05:** State lives in `SessionStore`, not in `AgentRuntime` or a new `RunningAgentRegistry` struct. Rationale: (a) `SessionStore` already owns per-session gateway state behind `Arc<RwLock<>>`, so no new locks; (b) the running-agent question is a gateway concern (one `AgentRuntime` per gateway process, many sessions, gateway needs per-session view); (c) keeps the change footprint inside the gateway crate; (d) `AgentRuntime` (`crates/ironhermes-agent/src/agent_runtime.rs:117`) stays channel-agnostic.

### Set/clear discipline

- **D-06:** The flag must be set `true` at the entry of `run_agent` and cleared `false` in a guaranteed-fire path covering every exit (success, error, panic-via-catch, cancellation token fired). The cleanest Rust idiom: a small RAII guard `RunningAgentGuard` that holds the `Arc<AtomicBool>` and clears it on `Drop`. Constructed at the top of `run_agent`, dropped at function exit. Eliminates the "forgot a cleanup branch" class of bug.

### Claude's Discretion

- Exact storage shape: a separate `HashMap<SessionKey, Arc<AtomicBool>>` on `SessionStore` vs. a new field on the existing `GatewaySession` struct (recommend the latter — keyed by `SessionKey` automatically, lifecycle matches the session). Planner picks.
- Whether the bypass-list check is a `match cmd.name { "stop" | "new" | "status" | "queue" => true, _ => false }` inline or a small `fn is_bypass(name: &str) -> bool` in `ironhermes-core::commands` (would be re-usable by CLI if/when it adopts the same mechanism). Planner picks.
- Where to place `RunningAgentGuard`: inside `crates/ironhermes-gateway/src/handler.rs` (private impl detail) vs. a new sibling module. Planner picks.
- Exact error message phrasing for D-02 / D-04 — minor wording within the "reject with error" intent.
- Whether to update the Phase 21.1 docstring at `handler.rs:377-380` in place vs. just delete the comment now that the gap is closed.

### Folded Todos

None — the three keyword-fuzzy matches from `gsd-sdk query todo.match-phase 36` (setup wizard scaffolding, Phase 18 UAT re-pass, configuration wizard improvements) are all generic gateway/configuration topics, not about the running-agent guard. Left in the backlog.

</decisions>

<canonical_refs>
## Canonical References

**Downstream agents MUST read these before planning or implementing.**

### Phase 21.1 review chain (origin of this phase)

- `.planning/phases/21.1-slash-commands/21.1-REVIEWS.md` — codex HIGH-1 (guard unspecified) and HIGH-2 (TOCTOU) — the literal motivation. Quote the findings in planning.
- `.planning/phases/21.1-slash-commands/21.1-CONTEXT.md` — D-09 (CommandContext shape), Pattern 3 (running-agent guard reference)
- `.planning/phases/21.1-slash-commands/21.1-RESEARCH.md` Pattern 3 — the hermes-agent guard pattern that was researched but not shipped
- `.planning/phases/21.1-slash-commands/21.1-02-PLAN.md` Task 2 — the original "TODO: Wire real agent_running state from session tracking" comment that this phase closes

### Implementation surfaces (the files this phase edits)

- `crates/ironhermes-gateway/src/handler.rs:62` — `GatewayMessageHandler` struct definition
- `crates/ironhermes-gateway/src/handler.rs:367-410` — current `handle_slash_command` entry, including the `agent_running = AtomicBool::new(false)` shim at lines 377-380 that this phase replaces
- `crates/ironhermes-gateway/src/handler.rs:801` — `run_agent` method; guard set/clear bracket this call
- `crates/ironhermes-gateway/src/handler.rs:1032` — locked comment "gateway has no per-turn cancel today" — confirms cancel-path is out of scope
- `crates/ironhermes-gateway/src/session.rs:11` — `SessionKey` (the key for per-session state)
- `crates/ironhermes-gateway/src/session.rs:87` — `SessionStore` (the home for per-session state per D-05)

### Command-router contract (unchanged by this phase but consumed by it)

- `crates/ironhermes-core/src/commands/context.rs:368` — `CommandContext.agent_running: Arc<AtomicBool>` (already correctly typed since Phase 21.1)
- `crates/ironhermes-core/src/commands/handlers.rs:127` — `cmd_stop` reads `ctx.agent_running.load(Ordering::SeqCst)`; this phase makes that read meaningful on gateway
- `crates/ironhermes-core/src/commands/handlers.rs:1507` — `cmd_queue` (TODO stub, bypassed but inert per D-01)
- `crates/ironhermes-core/src/commands/registry.rs` — bypass list MUST match command names registered here

### Phase 28.1 — AgentRuntime context (state-ownership tradeoff justification)

- `docs/AGENT-RUNTIME-DESIGN.md` — `AgentRuntime::run_turn` is the single channel-facing dispatch boundary; this phase deliberately keeps state OUT of AgentRuntime per D-05
- `.planning/phases/28.1-*/28.1-CONTEXT.md` and `28.1-02-PLAN.md` — gateway → run_turn migration (relevant if planner considers an alternative architecture)

### Requirement traceability

- `.planning/REQUIREMENTS.md` GW-05 (re-opened 2026-05-24, marked Partial) — primary requirement this phase closes

### External reference (port source)

- hermes-agent `gateway/run.py:1735-1852` — the bypass-list pattern this phase mirrors (D-01). Path: `/Users/twilson/code/hermes-agent/gateway/run.py`. Read for the original logic; do not literally port the queue-and-replay path (D-02 rejects instead).

</canonical_refs>

<specifics>
## Specific Ideas

- The exact comment to delete in `handler.rs:377-380` reads `"agent_running always false for gateway slash commands — the running-agent guard is a future enhancement using per-session state"`. After this phase, that comment is stale and either deleted or rewritten to point at the new mechanism.

- A `RunningAgentGuard` RAII type (D-06) is the cleanest implementation idiom. Sketch:
  ```rust
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
  Holding it across `run_agent` guarantees clear-on-exit even under `?` early-return.

- The `is_bypass(name)` predicate should ALSO accept resolved aliases. e.g. `/reset` resolves to `new`, so the predicate matches on the resolved `CommandDef.name`, not the raw user input. This is automatic because the guard check happens after `router.resolve(...)` returns an `Exact(def)` or `PrefixMatch(def)`.

- The guard check must come AFTER `router.resolve(...)` returns Exact/PrefixMatch and BEFORE calling `handlers::dispatch(...)`. Insert between those two existing calls in `handle_slash_command`. For `NotFound` (pass-through to agent per D-08 from Phase 21.1), the existing `run_agent` call is itself the non-bypass case — it should ALSO be guarded; otherwise the user can side-step the guard by typing a non-slash message. Hmm: actually, a non-slash message IS the user wanting to send input to the agent. Reject vs queue here matters separately from the slash-command guard. Planner should resolve: most likely also reject (with similar error), since per D-02 there's no input queue.

- Tests should cover the per-session isolation case explicitly: two `SessionKey`s, set one to Running, dispatch `/model` to the Idle one — must succeed. This catches "shared global flag" mistakes that codex HIGH-2 warned about.

- The `run_agent` method has multiple call sites in `handler.rs` (lines 523, 589, 772, 797, 1244). The guard is per-call inside `run_agent`, not per-call-site, so a single `RunningAgentGuard::new` at the top of `run_agent` covers all of them.

</specifics>

<deferred>
## Deferred Ideas

- **Per-turn LLM cancellation** (`handler.rs:1032`): real cancellation of an in-flight LLM call mid-turn. Required for richer state model (Idle/Running/Cancelling). Separate phase. When this lands, the guard's bypass behavior for `/stop` becomes meaningful — today `/stop` still only kills subagent processes, not the parent turn.

- **CLI parity unification**: today CLI sets its own `agent_running` flag in `main.rs` around `run_agent_turn`. Once the gateway has per-session state, a natural follow-up is to push this into a shared mechanism (perhaps an `AgentRuntime` field, perhaps a new `RunningAgentRegistry`) so CLI and gateway share one implementation. Out of scope here; the gateway-local fix per D-05 is the minimum-blast-radius solution.

- **Queue-and-replay UX** (D-02 alternative): hermes-agent's `gateway/run.py:1735-1852` queues messages received during an active turn and replays them after. Requires per-session pending-message buffer + replay hook in `run_agent` completion. Defer until UX feedback shows the reject-with-error UX is insufficient.

- **`/approve` / `/deny` bypass list addition** (D-01 alternative): add to bypass list when the approval queue is implemented. Tracked as a TODO in the bypass-list code so the next person extending it sees the marker.

- **`/model` defer-to-next-turn UX** (D-04 alternative): instead of rejecting, persist the new model preference into session config and apply on the next turn. Better UX; needs a session-config write path. Defer until rejection UX feedback warrants it.

- **`enum AgentState { Idle, Running, Cancelling, Queued }`** (D-03 alternative): the richer state model from the original phase rationale. Requires a per-turn cancel mechanism (for `Cancelling`) and an input-queue (for `Queued`) to have any observer. Deferred until those land.

### Reviewed Todos (not folded)

- "Add setup wizard and config scaffolding for gateway testing" — unrelated topic, scaffolding for `hermes setup`
- "Live UAT re-pass for Phase 18 behavioral tests 2-8" — unrelated, context-compression UAT
- "Configuration and setup wizard improvements" — Phase 23 / 35.1 territory, unrelated to this guard

</deferred>

---

*Phase: 36-gateway-running-agent-guard-wiring-completes-gw-05*
*Context gathered: 2026-05-24 via /gsd-discuss-phase*
