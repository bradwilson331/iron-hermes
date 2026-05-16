# Phase 34: Webchat & Multi-Platform Gateway Chats - Context

**Gathered:** 2026-05-15
**Status:** Ready for planning

<domain>
## Phase Boundary

Phase 34 delivers Learning Loop parity across all agent-execution surfaces beyond Telegram:

1. **Web UI nudge wiring** — `AppState.run_web_turn` gets a per-session nudge turn counter and fires `spawn_nudge_review` (Plan 32-03 lands in Phase 32 first; Phase 34 verifies parity is complete)
2. **Unified session store** — web chat sessions (`iron_hermes_ui`) are migrated to the same SQLite-backed `SessionStore` that the Telegram gateway uses, keyed by `Platform::Web + session_id`
3. **Discord adapter** — `DiscordAdapter` implements `PlatformAdapter` trait; routes Discord messages through `GatewayHandler.handle_with_multimodal` (inheriting nudge + skill-create automatically)
4. **Slack adapter** — `SlackAdapter` implements `PlatformAdapter` trait; same routing pattern

Phase 34 **depends on** Phase 32 and Phase 33 completing first — nudge infrastructure and `skill_manage` tool must exist before new surfaces are wired up.

</domain>

<decisions>
## Implementation Decisions

### Phase 32 — Web UI nudge wiring (Plan 32-03)
- **D-01:** Nudge turn counter lives on `AppState` as `nudge_turns: Arc<Mutex<HashMap<String, u32>>>` — mirrors the gateway's `nudge_turns: Arc<Mutex<HashMap<SessionKey, u32>>>` pattern exactly
- **D-02:** Fire site is inside `run_web_turn`, after `agent.run()` returns `Ok` — keeps nudge logic co-located with the agent call, mirrors `handle_with_multimodal`
- **D-03:** `tokio::spawn` fire-and-forget, same as CLI and gateway paths — `run_web_turn` must not block on nudge completion
- **D-04:** Plan 32-03 file location: `.planning/phases/32-periodic-nudge-memory-curation/32-03-PLAN.md` (wave 3, standalone plan)

### Phase 33 — Web path invariant test (Plan 33-03 update)
- **D-05:** Add `INV-33-07` to existing `invariants_33.rs` in Plan 33-03 Task 2 — verifies `AppState::new` calls `build_app_runtime_bundle` (confirming `skill_manage` is registered for web turns)
- **D-06:** No changes to `prompt_builder.rs` skill-creation trigger guidance — existing guidance is platform-agnostic; verify during Phase 34 research

### Session unification
- **D-07:** Web chat sessions must use the **same SQLite-backed `SessionStore`** as gateway sessions, keyed by `Platform::Web` + `session_id`
- **D-08:** `AppState` currently has its own `state_store` field — Phase 34 migrates this to share the singleton `SessionStore` used by the gateway
- **D-09:** Session identity across surfaces: a `Platform::Web` session and a `Platform::Telegram` session remain distinct — no cross-surface session merging in this phase

### Multi-platform adapters
- **D-10:** Discord adapter: implement `DiscordAdapter` for the `PlatformAdapter` trait — route through existing `GatewayHandler.handle_with_multimodal`
- **D-11:** Slack adapter: implement `SlackAdapter` for the `PlatformAdapter` trait — same routing
- **D-12:** Learning Loop coverage for Discord/Slack is **structural** — `handle_with_multimodal` already has nudge + skill-create wired; new adapters inherit it automatically; no per-adapter Learning Loop code needed
- **D-13:** No end-to-end UAT tests per platform in Phase 34 — integration tests confirming adapters route through `handle_with_multimodal` are sufficient

### Phase 34 ordering and scope
- **D-14:** Phase 34 depends on Phase 32 and Phase 33 completing first (sequential dependency)
- **D-15:** Phase 34 is numbered immediately after Phase 33 in the roadmap — phases 32 → 33 → 34 form the complete Learning Loop trilogy

### Claude's Discretion
- Which Discord/Slack API version and SDK library to use — researcher determines most tractable approach given existing `ironhermes-gateway` patterns
- Exact authentication/bot setup for Discord and Slack — left to planning phase
- Whether `SlackAdapter` uses Socket Mode (WebSocket) or Events API (HTTP) — researcher decides based on infrastructure fit

</decisions>

<canonical_refs>
## Canonical References

**Downstream agents MUST read these before planning or implementing.**

### Learning Loop (phases 32 & 33 — must complete before Phase 34 execution)
- `.planning/phases/32-periodic-nudge-memory-curation/32-01-PLAN.md` — nudge infrastructure (MemoryConfig, nudge.rs, run_chat wiring)
- `.planning/phases/32-periodic-nudge-memory-curation/32-02-PLAN.md` — gateway nudge wiring (GatewayHandler.nudge_turns, handle_with_multimodal fire site)
- `.planning/phases/32-periodic-nudge-memory-curation/32-03-PLAN.md` — web UI nudge wiring (AppState.nudge_turns, run_web_turn fire site) — **must be created as part of Phase 32 scope update**
- `.planning/phases/33-autonomous-skill-creation/33-02-PLAN.md` — SkillManageTool implementation
- `.planning/phases/33-autonomous-skill-creation/33-03-PLAN.md` — learning toolset wiring + INV-33-07 (web path invariant)

### Agent execution surfaces (the three paths that need Learning Loop coverage)
- `crates/ironhermes-cli/src/main.rs` — `run_chat` REPL path (CLI)
- `crates/ironhermes-gateway/src/handler.rs` — `handle_with_multimodal` (Telegram gateway; future Discord/Slack routes here too)
- `crates/iron_hermes_ui/src/server/state.rs` — `AppState::run_web_turn` (web UI WebSocket path)
- `crates/iron_hermes_ui/src/server/ws.rs` — WebSocket connection handler that calls `run_web_turn`

### Platform adapter infrastructure
- `crates/ironhermes-gateway/src/adapter.rs` — `PlatformAdapter` trait definition; new adapters implement this
- `crates/ironhermes-gateway/src/telegram.rs` — `TelegramAdapter` implementation (reference pattern for Discord/Slack)
- `crates/ironhermes-gateway/src/handler.rs` — `GatewayHandler` struct; nudge_turns + handle_with_multimodal are the integration points

### Session infrastructure
- `crates/iron_hermes_ui/src/server/state.rs` — `AppState::new` + `ensure_web_session` + `run_web_turn` (current web session path — needs unification)
- Session key pattern: `SessionKey::new(Platform::Web, session_id)` — must match gateway's `SessionKey::new(Platform::Telegram, chat_id)` pattern

</canonical_refs>

<code_context>
## Existing Code Insights

### Reusable Assets
- `nudge_turns: Arc<Mutex<HashMap<SessionKey, u32>>>` on `GatewayHandler` — exact pattern to mirror on `AppState` with `String` key (web session_id)
- `spawn_nudge_review` in `crates/ironhermes-agent/src/nudge.rs` — already implemented in Phase 32; just needs a new call site in `run_web_turn`
- `PlatformAdapter` trait in `crates/ironhermes-gateway/src/adapter.rs` — Discord and Slack adapters implement this; no changes to the trait needed
- `TelegramAdapter` in `crates/ironhermes-gateway/src/telegram.rs` — reference implementation for new platform adapters

### Established Patterns
- Gateway's nudge fire site (Plan 32-02 interfaces section) — lock `nudge_turns` in sync block, extract `should_fire` bool, drop lock, then `tokio::spawn`; must NOT hold `Mutex` guard across `.await`
- `AppState` uses `build_app_runtime_bundle` (confirmed in `state.rs:83`) — `skill_manage` is already registered for web turns; no factory changes needed for Phase 33 coverage
- `AppState.memory_manager` is already wired into `attach_context_engine` — available to pass into `spawn_nudge_review`

### Integration Points
- `run_web_turn` → add nudge counter check + `tokio::spawn(spawn_nudge_review(...))` after `agent.run().await?` returns
- `AppState::new` → add `nudge_turns: Arc::new(Mutex::new(HashMap::new()))` to struct initialization
- New `DiscordAdapter`/`SlackAdapter` → registered alongside `TelegramAdapter` in the gateway startup path
- Unified session store: `AppState.state_store` migrated to share the gateway's singleton `SessionStore`

</code_context>

<specifics>
## Specific Ideas

- "Web sessions will need to be wired up to real sessions, no matter where they are from" — the unified `SessionStore` migration is a first-class deliverable, not a nice-to-have
- Platform::Web sessions must be keyed consistently so the nudge counter HashMap key matches the session store key
- Discord and Slack adapters ship in Phase 34; the Learning Loop works on them automatically via `handle_with_multimodal` — no extra Learning Loop code per adapter

</specifics>

<deferred>
## Deferred Ideas

- Cross-surface session sharing (Telegram session resumed in web UI with same session_id) — belongs in a dedicated session-continuity phase after Phase 34
- WhatsApp or other platform adapters beyond Discord/Slack — deferred to future phases
- End-to-end UAT per platform (send 10 Discord messages, verify nudge fires) — deferred; structural invariant tests are sufficient for Phase 34
- `Platform::Web` variant in skill-creation trigger guidance (prompt_builder.rs) — defer; existing guidance is platform-agnostic; verify during Phase 34 research and add only if needed

</deferred>

---

*Phase: 34-Webchat-and-Multi-Platform-Gateway*
*Context gathered: 2026-05-15*
