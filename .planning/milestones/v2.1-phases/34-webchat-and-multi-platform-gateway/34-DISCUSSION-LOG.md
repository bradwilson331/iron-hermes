# Phase 34: Webchat & Multi-Platform Gateway Chats - Discussion Log

> **Audit trail only.** Do not use as input to planning, research, or execution agents.
> Decisions are captured in CONTEXT.md — this log preserves the alternatives considered.

**Date:** 2026-05-15
**Phase:** 34-webchat-and-multi-platform-gateway
**Areas discussed:** Web nudge counter placement, Phase 34 scope boundary, Plan update strategy, Multi-platform gateway coverage, Phase 34 dependencies, ROADMAP placement, Plan 32-03 file location, Web session infrastructure

---

## Web Nudge Counter Placement

| Option | Description | Selected |
|--------|-------------|----------|
| On AppState | Add nudge_turns: Arc<Mutex<HashMap<String, u32>>> to AppState — mirrors the gateway pattern exactly; persists across WebSocket reconnects | ✓ |
| Connection-local in ws.rs | Track turns_since_nudge as a plain u32 in the ws_chat connection loop — simpler, no Arc/Mutex; resets to 0 on every browser reconnect | |
| In session store (SQLite) | Persist the counter to the SQLite session store — survives reconnects but adds a DB round-trip per turn | |

**User's choice:** On AppState
**Notes:** Mirrors gateway pattern exactly. Fire site confirmed as inside `run_web_turn`, after `agent.run()` returns Ok — keeps nudge logic co-located with the agent call.

---

## Where spawn_nudge_review Is Called From

| Option | Description | Selected |
|--------|-------------|----------|
| Inside run_web_turn | After agent.run() returns Ok in run_web_turn — mirrors handle_with_multimodal | ✓ |
| In ws.rs after awaiting run_web_turn | ws.rs checks and fires separately — separates concerns but adds surface area | |

**User's choice:** Inside run_web_turn

---

## Phase 34 Scope Boundary

| Option | Description | Selected |
|--------|-------------|----------|
| Learning Loop parity for web + multi-platform | Phase 34 = web nudge wiring + web session unification + Discord + Slack adapters | ✓ |
| Web-only Learning Loop parity | Phase 34 = only the web UI gap; multi-platform left for later | |
| Multi-platform adapters only | Phase 34 = Discord/Slack adapters; web nudge fix goes into Phase 32 directly | |

**User's choice:** Learning Loop parity for web + multi-platform

---

## New Platform Adapters

| Option | Description | Selected |
|--------|-------------|----------|
| Discord | Implement DiscordAdapter for PlatformAdapter trait | ✓ |
| Slack | Implement SlackAdapter — Slack Events API / Socket Mode | ✓ |
| You decide | Let research determine which adapter(s) are tractable | |

**User's choice:** Discord and Slack

---

## Plan Update Strategy

| Option | Description | Selected |
|--------|-------------|----------|
| New Plan 32-03 | Standalone 32-03-PLAN.md for web UI nudge wiring | ✓ |
| Expand Plan 32-02 | Update existing 32-02-PLAN.md to cover both gateway + web UI | |

**User's choice:** New Plan 32-03 at `.planning/phases/32-periodic-nudge-memory-curation/32-03-PLAN.md`

---

## Phase 33 Web Coverage

| Option | Description | Selected |
|--------|-------------|----------|
| Add INV-33-07 to Plan 33-03 | Update Plan 33-03 Task 2 to add 7th invariant test (web path) | ✓ |
| New Plan 33-04 | Standalone plan for web coverage | |

**User's choice:** INV-33-07 added to existing `invariants_33.rs` in Plan 33-03

---

## Multi-Platform Gateway Coverage

| Option | Description | Selected |
|--------|-------------|----------|
| Structural — no extra work needed | handle_with_multimodal inherits nudge + skill-create; new adapters get it automatically; integration tests confirm routing | ✓ |
| Explicit per-adapter invariant tests | Add INV-34-* tests per adapter | |
| End-to-end UAT per platform | Manual UAT for Discord and Slack | |

**User's choice:** Structural — no per-adapter Learning Loop code needed

---

## Phase 34 Dependencies

| Option | Description | Selected |
|--------|-------------|----------|
| Depends on Phase 32 and 33 | Sequential: 32 → 33 → 34 | ✓ |
| Partial — web session wiring starts now | Web session infrastructure independent of nudge/skill-create | |
| No dependency — fully parallel | Plans can be written now, execution gates later | |

**User's choice:** Full dependency on Phase 32 and Phase 33

---

## ROADMAP Placement

| Option | Description | Selected |
|--------|-------------|----------|
| Immediately after Phase 33 | Phases 32 → 33 → 34 form the complete Learning Loop trilogy | ✓ |
| After a gap phase (33.1) | Insert web session infra phase between 33 and 34 | |
| Numbered differently (34.0 or 32.2) | Sub-phase numbering to signal tight coupling | |

**User's choice:** Immediately after Phase 33 as Phase 34

---

## Web Session Infrastructure ("real sessions")

| Option | Description | Selected |
|--------|-------------|----------|
| Unified session store | Web sessions use same SQLite-backed SessionStore as gateway (Platform::Web + session_id) | ✓ |
| Same session_id namespace | Cross-surface session continuity (resume Telegram session in web UI) | |
| Platform::Web in metadata only | Web sessions already have Platform::Web; "real" means auth/allowlist checks | |

**User's choice:** Unified session store — `AppState.state_store` migrated to share the gateway's singleton `SessionStore`

---

## prompt_builder Skill-Creation Guidance

| Option | Description | Selected |
|--------|-------------|----------|
| No — guidance is platform-agnostic already | Existing LEARN-03 guidance doesn't hardcode telegram; verify during research | ✓ |
| Yes — update for web platform | Add Platform::Web context to trigger guidance | |

**User's choice:** No changes; verify during Phase 34 research

---

## Claude's Discretion

- Which Discord/Slack API version and SDK library to use — researcher determines most tractable approach given existing `ironhermes-gateway` patterns
- Whether `SlackAdapter` uses Socket Mode (WebSocket) or Events API (HTTP) — researcher decides based on infrastructure fit
- Exact bot authentication/setup for Discord and Slack

## Deferred Ideas

- Cross-surface session sharing (Telegram session resumed in web UI with same session_id) — belongs in a dedicated session-continuity phase after Phase 34
- WhatsApp or additional platform adapters beyond Discord/Slack — future phases
- End-to-end UAT per platform (send 10 Discord messages, verify nudge fires) — deferred; structural invariant tests sufficient for Phase 34
- Platform::Web in skill-creation trigger guidance — defer; verify during research
