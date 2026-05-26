# Phase 36 — Discussion Log

**Date:** 2026-05-24
**Mode:** /gsd-discuss-phase (default — 4 single-question turns)
**Triggered by:** /gsd-plan-phase 36 → no CONTEXT.md → user chose "Run discuss-phase first"

## Pre-discussion context loaded

- PROJECT.md (IronHermes overview, v2.0/2.1 history)
- REQUIREMENTS.md — GW-05 re-opened 2026-05-24 (Partial — guard pending)
- ROADMAP.md — Phase 36 entry with codex-flagged rationale in Goal field
- Phase 21.1 artifacts: CONTEXT.md, RESEARCH.md (Pattern 3 = guard pattern), 02-PLAN.md, REVIEWS.md (codex HIGH-1 + HIGH-2)
- Codebase maps: ARCH.md, CONCERNS.md, QUALITY.md, TECH.md
- Targeted grep: handler.rs:377-380 (the hardcoded `agent_running=false` shim), handler.rs:801 (run_agent), session.rs:11/87 (SessionKey / SessionStore), handlers.rs:127 (cmd_stop), handlers.rs:1507 (cmd_queue TODO), context.rs:368 (CommandContext.agent_running typed Arc<AtomicBool> already), agent_runtime.rs:117 (AgentRuntime — Phase 28.1)

## Folded todos

None. Three keyword-fuzzy todo matches were all unrelated (setup wizard scaffolding, Phase 18 UAT, configuration wizard improvements).

## Discussion areas presented

Four phase-specific gray areas were presented (single AskUserQuestion batch with four questions). No "skip" option was offered — user explicitly invoked discuss-phase.

### Area 1 — Bypass list

**Question:** Which slash commands should BYPASS the running-agent guard on gateway?

**Options presented:**
1. (Recommended) hermes-agent parity: `/stop /new /status /queue` — mirrors `gateway/run.py:1735-1852`. `/queue` is partially wired (TODO message at handlers.rs:1507) but listing preserves parity for future infrastructure.
2. Narrow: `/stop /new /status` (no `/queue`) — only fully wired commands.
3. Wider: above + `/approve /deny` — future-proof for approval queue.

**User selection:** Option 1 — hermes-agent parity.

**Notes:** Captured as D-01. `/approve` and `/deny` deliberately excluded because they're TODO stubs with no backing infrastructure; adding them later when the approval queue lands is captured as a deferred idea.

### Area 2 — Non-bypass UX

**Question:** When a non-bypass command arrives during an active agent turn, what should the gateway do?

**Options presented:**
1. (Recommended) Reject with explanatory error — send "Agent is running. Use /stop to interrupt or /queue to send after this turn."
2. Queue and auto-replay after turn completes — needs new pending-messages structure.
3. Interrupt + replace — requires per-turn cancel that's out-of-scope this phase.

**User selection:** Option 1 — reject with explanatory error.

**Notes:** Captured as D-02. Queue-and-replay UX captured as a deferred idea; can be added later if rejection UX feedback shows insufficient.

### Area 3 — State model

**Question:** What state model should per-session running-agent state use?

**Options presented:**
1. (Recommended) Bool agent_running per session — `HashMap<SessionKey, AtomicBool>` in SessionStore.
2. Enum: Idle | Running | Cancelling — pays for future cancellation work.
3. Enum: Idle | Running | Cancelling | Queued — full state machine matching original rationale; requires queue infra to be meaningful.

**User selection:** Option 1 — bool per session.

**Notes:** Captured as D-03. Note: this slightly narrows the original Phase 36 rationale in ROADMAP.md ("Replace `Arc<AtomicBool>` shim with per-session state (Idle/Running/Cancelling/Queued)") — the user explicitly chose the simpler bool model because the richer enum has no observer until per-turn cancel and an input queue land. Enum upgrade captured as a deferred idea.

### Area 4 — /model mid-turn behavior

**Question:** Codex flagged `/model` mid-turn as TOCTOU-prone (credentials swap during in-flight API call). What's the desired behavior?

**Options presented:**
1. (Recommended) Reject with error — "/model during Running returns 'Agent is running. /stop first to change model.'"
2. Defer: write to session config, take effect next turn.
3. Special-case: allow only if same provider (cred-safe).

**User selection:** Option 1 — reject with error.

**Notes:** Captured as D-04. Closes codex HIGH-2 directly. Defer-to-next-turn UX captured as a deferred idea; same-provider special-case rejected as adding non-trivial complexity for niche benefit.

## Implicit decisions captured during discussion

### State ownership location (D-05)

State lives in `SessionStore` (gateway-local), not `AgentRuntime` or a new `RunningAgentRegistry`. Implicit from D-03 — bool-per-session-keyed-by-SessionKey naturally fits `SessionStore` which already owns per-session gateway state behind `Arc<RwLock<>>`. Rationale logged in CONTEXT.md.

### Set/clear discipline (D-06)

RAII guard pattern: a small `RunningAgentGuard` type that sets `true` on construction and `false` on `Drop`, held across `run_agent`. Not explicitly asked but logged as Claude's-Discretion-with-strong-recommendation in CONTEXT.md because the alternative (manual clear in every exit branch) is a known footgun and the user's preference for simple state (D-03) implies preferring a simple bulletproof set/clear pattern.

## Scope-creep deflections

None encountered. User stayed on-domain across all four questions. The user did note that the original ROADMAP.md Goal mentioned the richer Idle/Running/Cancelling/Queued enum — when offered that option (Area 3, option 3), they chose the simpler bool model, which is a legitimate scope narrowing (the enum needs observers that don't exist) rather than scope creep.

## Carried-forward decisions referenced

- From Phase 21.1: `CommandContext.agent_running: Arc<AtomicBool>` is already correctly typed (context.rs:368). This phase populates it correctly; no API change needed in `ironhermes-core`.
- From Phase 21.1 D-08: Unknown commands pass through to agent as normal messages. Phase 36 inherits this; the guard ALSO applies on the pass-through path (specifics §5).
- From Phase 28.1: `AgentRuntime::run_turn` is the channel-facing dispatch boundary. State could have lived here but doesn't (D-05) — keeping AgentRuntime channel-agnostic.

## Out-of-scope items captured as deferred ideas

1. Per-turn LLM cancellation (the `handler.rs:1032` "gateway has no per-turn cancel today" gap)
2. CLI parity unification (CLI has its own agent_running flag; merging is a follow-up)
3. Queue-and-replay UX (D-02 alternative)
4. `/approve` / `/deny` bypass list addition (D-01 alternative; depends on approval queue)
5. `/model` defer-to-next-turn UX (D-04 alternative)
6. `enum AgentState { Idle, Running, Cancelling, Queued }` (D-03 alternative; needs observers)

---

*Generated 2026-05-24 by /gsd-discuss-phase 36*
