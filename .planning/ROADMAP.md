### Phase 28.1: AgentRuntime channel migration (budget/skills/tools ownership) per docs/AGENT-RUNTIME-DESIGN.md (INSERTED)

**Goal:** `AgentRuntime` is the single channel-facing agent API: every channel (Telegram gateway, web UI, CLI `run_chat`/`run_single`, TUI) builds one `AgentRuntime` and calls `run_turn(TurnRequest)` per top-level turn. No channel constructs `BudgetHandle`s or assembles `AgentLoop`s by hand; the run-boundary owns budget reset, permanently fixing the `Stop100` latch class for current and future channels. Cron gets a separate runtime/budget so scheduled turns do not drain interactive chat.
**Requirements**: AGENT-RUNTIME-MIGRATION (scope + locked decisions §6 in docs/AGENT-RUNTIME-DESIGN.md and 28.1-CONTEXT.md)
**Depends on:** Phase 28
**Plans:** 6/6 plans complete

Plans:

- [x] 28.1-01-PLAN.md — AgentRuntime budget-reset regression test (foundational proof; agent crate)
- [x] 28.1-02-PLAN.md — Gateway → run_turn; remove 367eaa79 band-aid (highest value)
- [x] 28.1-03-PLAN.md — Web UI → run_turn; close top-level-loop budget gap
- [x] 28.1-04-PLAN.md — CLI run_chat + run_single → run_turn; fix run_chat latch
- [x] 28.1-05-PLAN.md — TUI → run_turn; fix latch + max_turns/max_iterations drift
- [x] 28.1-06-PLAN.md — Cron distinct runtime/budget (§6.4); preserve per-job overrides

**Note:** Stage 4 (skills + tool-registry ownership fully into AgentRuntime, design §4) is intentionally DEFERRED to a follow-up phase — see planning summary. It would edit the same channel files this phase migrates and is independently shippable per §5.

### Phase 34b: Context-system parity (@-references + ContextEngine lifecycle + Compressor reset)

**Goal:** Close the parity gap with three hermes-agent context-system modules, wired into the post-28.1 `AgentRuntime::run_turn` chokepoint. (1) `@`-reference expansion (`context_references.py`): users write `@file:/@folder:/@diff/@staged/@git:N/@url:` in chat; tokens are parsed, expanded into a bounded `--- Attached Context ---` footer, and stripped from the inline message — preprocessed ONCE centrally in `run_turn` (D-09/D-11) with a sensitive-path blocklist (.ssh/.aws/.env/etc.) and a 50% hard / 25% soft token budget; expansion warnings ride back on `AgentResult.context_warnings` so all three surfaces render the `--- Context Warnings ---` block. (2) `ContextEngine` lifecycle hook parity (`context_engine.py`): 5 additive default-no-op hooks (`on_session_start`, `on_session_reset`, `update_from_response`, `update_model`, `has_content_to_compress`); per-turn hooks fire once centrally in `run_turn`, per-session reset stays at the surfaces. (3) `ContextCompressor` counter reset on `/new` + memory-authority reminder ("MEMORY.md … ALWAYS authoritative") in the compaction header. D-10 resolved via the existing `compression_count` state-threading precedent (surface-owned durable counter; engine rebuilt fresh per turn).
**Requirements**: CTX-REF-W0, CTX-ENG-W0, CTX-REF-01, CTX-REF-02, CTX-ENG-01, CTX-ENG-02, CTX-ENG-03, CTX-ENG-04 (phase-local; defined during /gsd:discuss-phase 34b)
**Depends on:** Phase 34a (read-side memory parity), Phase 28.1 (AgentRuntime run_turn chokepoint)
**Plans:** 4/4 plans complete

Plans:
**Wave 0**

- [x] 34B-00-PLAN.md — Test scaffolds: context_refs module stub, invariants_34b, #[ignore] reset + memory-authority placeholders

**Wave 1** *(depends on Wave 0)*

- [x] 34B-01-PLAN.md — @-reference expansion module (parser + expander + sensitive-path blocklist + 50%/25% budget) + central run_turn preprocessing + AgentResult.context_warnings carrier (D-09/D-11)

**Wave 2** *(depends on Wave 1)*

- [x] 34B-02-PLAN.md — ContextEngine 5 lifecycle hooks + ContextCompressor reset + memory-authority reminder + central per-turn hook in run_turn + surface session-reset wiring (D-09/D-10)

**Wave 3** *(gap closure — WR-01)*

- [x] 34b-03-PLAN.md — Close WR-01: stop in-message `--- Context Warnings ---` embedding, wire CLI/gateway/web to render `AgentResult.context_warnings` out-of-band, correct doc comments, source-guard test

### Phase 35: Per-subagent independent iteration budgets (retire PROV-10; T-28.1-16)

**Goal:** Replace IronHermes' PROV-10 shared parent↔child budget with **per-subagent independent iteration budgets**, matching the hermes-agent reference. Each subagent (interactive and cron) is given a fresh `BudgetHandle::new(delegation.max_iterations)` (already default 50) in `AgentSubagentRunner` instead of a clone of the parent's budget Arc, so a child can no longer decrement its parent's counter. Runaway delegation is bounded by `max_spawn_depth × max_concurrent_children × delegation.max_iterations` rather than one shared counter; the threat model and PROV-10 regression tests are updated accordingly. T-28.1-16 (cron subagents draining the interactive budget via the shared `ToolRegistry` delegate runner) is resolved as a consequence — with no shared parent/child counter, cron fan-out cannot touch interactive headroom.
**Requirements**: T-28.1-16 (from Phase 28.1). NOTE: §8's cron-specific fix is superseded by the global per-subagent model — see 35-CONTEXT.md. Gap described in docs/AGENT-RUNTIME-DESIGN.md §6.4 / §8.
**Depends on:** Phase 28.1 (AgentRuntime channel migration — cron distinct top-level budget shipped in 28.1-06)
**Plans:** 4 plans (3 complete + 1 gap-closure pending)

Plans:
**Wave 1**

- [x] 35-01-PLAN.md — Clamp delegate_task max_iterations to the config ceiling (D-03 Option B) + rewrite override test
- [x] 35-02-PLAN.md — Fresh per-child BudgetHandle at the runner change site; retire PROV-10 parent↔child counter; D-07.1 independence test

**Wave 2** *(blocked on Wave 1 completion)*

- [x] 35-03-PLAN.md — Subagent-layer cron independence test (T-28.1-16 acceptance); amend AGENT-RUNTIME-DESIGN.md §6.4/§8 + threat model

### Phase 35.1: hermes-agent install and setup parity (INSERTED)

**Goal:** Make `ironhermes setup` result in a fully working agent — matching the hermes-agent end-to-end setup experience. Close the six actual gaps in the existing wizard/preflight code (CFG-02 and CFG-03 already shipped in `config_cli.rs` and are NOT re-implemented): (1) replace the `bail!("section deferred to Phase 28")` at `setup.rs:122` with a real `run_skills_section` mirroring `run_tools_section` (D-01); (2) add a `run_terminal_section` that prompts for `cwd` only (D-02); (3) add a "Quick vs Full" choice prompt at wizard entry (D-11) so the fast path is the default; (4) call `doctor::run_doctor_check()` automatically at wizard exit as the final preflight gate (D-03), extracting `cmd_doctor` from `main.rs` into a new `src/doctor.rs` module; (5) print a completion summary with configured provider, model, enabled platforms, and a next-step hint (D-12); (6) add D-07/D-08 first-run LLM detection to `preflight.rs` — auto-launch wizard when no API key (OPENROUTER/ANTHROPIC/OPENAI) is set AND no localhost/127.0.0.1 base_url is configured, with `l.len() > key.len()` guard against empty-value bypass (T-35.1-01). The Phase 23 preflight outer gate condition (`Chat | Gateway | None`) stays byte-for-byte LOCKED — D-08 is added inside the existing valid-config branch only.
**Requirements**: CFG-01 (active wizard work), CFG-02 (already satisfied in `config_cli.rs::cmd_config_set/get/show`), CFG-03 (already satisfied in `config_cli.rs::cmd_config_migrate`)
**Depends on:** Phase 35
**Plans:** 5/6 plans executed

Plans:
**Wave 0**

- [x] 35.1-00-PLAN.md — Extract `cmd_doctor` to `src/doctor.rs` module + create Wave 0 test scaffolds (`tests/setup_wizard.rs` with 6 #[ignore] stubs; extend `tests/doctor_integration.rs` with d07_d08 stub)

**Wave 1** *(depends on Wave 0)*

- [x] 35.1-01-PLAN.md — Implement `run_skills_section` (D-01) + `run_terminal_section` (D-02) + `apply_skills_prereq_answers` testability seam; replace the bail line at setup.rs:122; un-ignore D-01 and D-02 tests

**Wave 2** *(parallel: 02 and 03 have zero `files_modified` overlap — 02 touches setup.rs + tests/setup_wizard.rs; 03 touches preflight.rs + tests/doctor_integration.rs)*

- [x] 35.1-02-PLAN.md — Wire D-11 fast/full choice + D-03 in-process doctor call + D-12 completion summary into `run_setup` None arm; un-ignore d11/d03/d12 source-text invariant tests
- [x] 35.1-03-PLAN.md — Implement `has_runnable_llm` helper (env-vars → raw .env → local base_url ordering) + integrate into `preflight.rs` Ok(config) arm (D-07/D-08); un-ignore d07_d08 integration tests; verify main.rs Phase 23 gate stays byte-for-byte unchanged

---

## Milestone v3.0: Hermes-agent parity

**Declared:** 2026-05-25 (retroactive label over 36.x phases)
**Goal:** Close the breadth gap between ironhermes (Rust) and hermes-agent (Python v0.14.0) along selected axes — agent loop, tools, skills library, LLM providers, TUI, multi-platform gateway, ACP, MCP server, memory/state, configuration/secrets, packaging — while explicitly rejecting the long tail (17 deferred messaging platforms; no plugin loader port).
**Source of truth:** `/Users/twilson/Documents/iron-hermes-planning.md` (parity comparison, 2026-05-24)
**Phases:** 14 parents + 20 sub-phases = 34 total. Phase 36 + 36.1 carry over from v2.1 (36.1 SHIPPED 2026-05-25). Phases 36.2 through 36.13 inserted during the parity walk on 2026-05-25.
**Strategic narrowings (referenced from memory):**
- `project_multiplatform_gateway_scope` — 17 messaging platforms deferred under Phase 36.7
- `project_plugin_loader_rejected` — Phase 36.13 ships AgentRuntime primitives instead of a loader

### Phase 36: Gateway running-agent guard wiring — completes GW-05

**Goal:** Wire per-session running-agent state on the gateway so `/stop`, `/approve`, `/deny` bypass while `/model` and other state-mutating commands are queued during an active agent turn. Cross-AI review of Phase 21.1 (2026-05-24, codex HIGH-1) confirmed `crates/ironhermes-gateway/src/handler.rs:377-380` hardcodes `agent_running = AtomicBool::new(false)` with the comment "running-agent guard is a future enhancement using per-session state" — leaving GW-05 only partially satisfied: dispatch through `resolve_command()` works, but the guard never fires, `/stop` always reports "no agent running" on Telegram, and `/model` can switch credentials mid-turn. Replace the per-request `Arc<AtomicBool>` shim with per-session state (Idle/Running/Cancelling/Queued) keyed by `SessionKey`, threaded into `CommandContext`, to eliminate TOCTOU races between flag check and dispatch. Mirror hermes-agent's `gateway/run.py:1735-1852` bypass list (`/stop`, `/new`, `/queue`, `/status`).
**Requirements**: GW-05 (re-opened 2026-05-24 — see REQUIREMENTS.md note + `.planning/phases/21.1-slash-commands/21.1-REVIEWS.md` HIGH-1/HIGH-2)
**Depends on:** Phase 35
**Plans:** 3/3 plans complete

Plans:
**Wave 1**

- [x] 36-01-PLAN.md — Wave 0 test scaffold: `crates/ironhermes-gateway/tests/running_agent_guard_tests.rs` with 11 `#[ignore]` GW-05 sub-behavior stubs + helpers (RecordingPlatformAdapter, build_test_session_store, d02_error_message)

**Wave 2** *(depends on Wave 1)*

- [x] 36-02-PLAN.md — Core implementation: add `running: Arc<AtomicBool>` to `GatewaySession` (D-03/D-05) + `SessionStore::get_running_flag` accessor; add `RunningAgentGuard` RAII (D-06) and `is_bypass` (D-01) to handler.rs; wire guard at `run_agent` top + `handle_slash_command` + non-slash `MessageHandler::handle` AND `handle_with_multimodal` (Pitfall 1); un-ignore all 11 tests

**Wave 3** *(depends on Wave 2)*

- [x] 36-03-PLAN.md — Cleanup: delete stale "future enhancement" comment at handler.rs:377-380; flip REQUIREMENTS.md GW-05 to Complete + traceability row to "Phase 21.1 (dispatch) + Phase 36 (guard)"; update ROADMAP.md checkboxes; create 36-BACKLOG.md (web UI slash-interception gap; per-turn LLM cancel handler.rs:1032; CLI/gateway unified mechanism; /approve+/deny bypass when approval queue lands); Real-Telegram UAT checkpoint

### Phase 36.17: iron_hermes_ui web logging in $IRONHERMES_HOME/logs (COMPLETE)

**Goal:** Mirror the TUI file-logging pattern (commit eedb49e1) in the `iron_hermes_ui` server binary: daily-rolling `web.log` (app/agent tracing) and `web-access.log` (HTTP access via `tower_http::trace::TraceLayer`) under `$IRONHERMES_HOME/logs/`, with ANSI-stripped file output, non-blocking writers held across `axum::serve`, per-layer EnvFilters, and console behavior unchanged.
**Requirements**: D-01..D-18 (see 36.17-CONTEXT.md — phase uses D-IDs in lieu of REQ-IDs)
**Depends on:** Phase 36
**Plans:** 4/4 plans complete · UAT 5/5 green (2026-05-27) · post-execute fixes: `2df0ae60` (UAT public-dir precondition) · `2ab57e72` (production graceful-shutdown + startup INFO marker)

Plans:
- [x] 36.17-01-PLAN.md — Add `tower-http = { version = "0.6", features = ["trace"] }` to workspace deps; add `tracing-appender` + `tower-http` to `iron_hermes_ui` non-wasm32 deps (D-13/D-14)
- [x] 36.17-02-PLAN.md — Create `crates/iron_hermes_ui/src/server/logging.rs` with `install_web_logger_subscriber()` (3-layer registry, both appenders, ANSI-stripped file layers, per-layer filters, `try_init`); declare module in `server/mod.rs` (D-02..D-05, D-15..D-18)
- [x] 36.17-03-PLAN.md — Replace `tracing_subscriber::fmt().init()` in `main.rs` with `install_web_logger_subscriber()`; mount `TraceLayer::new_for_http().on_request(DefaultOnRequest::new().level(Level::INFO)).on_response(DefaultOnResponse::new().level(Level::INFO))` on the Axum router (D-01, D-07..D-12 + Q2 INFO-level fix)
- [x] 36.17-04-PLAN.md — Create `scripts/uat/phase-36.17-web-logging.sh` UAT script (mktemp IRONHERMES_HOME, start server, curl, assert both files exist + non-empty + ANSI-free + `tower_http::trace` target present in access log); blocking-human verify (D-01..D-05, D-09, D-10, D-15, D-16)

### Phase 36.17.4: wire up iron_hermes_ui to the gateway queue + slash commands (INSERTED)

**Goal:** Bring real FIFO queueing to the iron_hermes_ui Dioxus web surface so `/queue <message>` and the queue-aware slash commands behave the same way they already do on Telegram (post-36.17.2.1) and the ratatui TUI (post-36.17.3). Replace the in-flight reject at `crates/iron_hermes_ui/src/server/ws.rs:204-214` with `app_state.queue.try_push(&key, message)` keyed by `SessionKey { platform: Web, chat_id: session_id, user_id: Some("web") }` (D-01); auto-drain via the existing WS recv-loop `None =>` branch in arm 2 after the spawned turn's tx channel closes (D-02 — RESEARCH Finding 1 simplest Option A variant; no new tokio::Notify, no fourth select arm); typed event `ChatStreamEvent::QueueUpdated { depth: u32, paused: bool }` with locked external-tagged JSON `{"QueueUpdated":{"depth":3,"paused":false}}` (D-03 / D-11); wasm UI ships `Queue: N` / `Queue: N (paused)` pill in HermesApp's AppFooter driven by `Signal<(u32, bool)>` with hide-when-zero discipline (D-03a / D-03b); /new and /reset clear queue + reset paused + emit QueueUpdated BEFORE `reset_web_session` (D-04); /stop intercepted early — `queue.clear` → `paused.store(false, SeqCst)` → emit QueueUpdated → Delta `"Queue cleared. Current turn finishing.\n"` → Finished — **with NO `JoinHandle::abort` and NO `cancel_token.cancel()`** (D-04a / D-05 — documented divergence from 36.17.3 D-08 and 36.17.2 /stop semantics; in-flight cancel deferred since AppState has no per-session CancellationToken); cap = 128 with Delta `"Queue is full (128/128). /stop or /flush to drain.\n"` (D-06, terminal bell omitted — browser not TTY); /unpause alias (NOT /resume — registry collision per 36.17.3 D-06); RESEARCH Finding 4 critical gap closed at the core level by extending `ironhermes_core::commands::running_agent::is_bypass` to include `pause` and `unpause` (D-08 invariant). New integration test triplet `tests/web_queue_drain.rs` (FIFO + cap + paused-blocks-drain unit tests + state.rs/ws.rs source anchors) / `tests/web_queue_controls.rs` (D-04 / D-04a / D-05 / D-06 / D-08 source-text + pause/unpause AtomicBool state-machine) / `tests/web_queue_protocol.rs` (protocol variant + mod.rs signal/arm + app_footer.rs pill anchors) — D-09 split mirrors the 36.17.3 triplet; D-10 highest-leverage seam = `include_str!` + assert-contains + direct `SessionQueue` unit tests (no Axum spin-up). Phase gate D-12: full `cargo test -p iron_hermes_ui && cargo test -p ironhermes-gateway && cargo test -p ironhermes-cli --features test-support && cargo check --workspace --all-features` green. Out of scope (carried forward): in-flight turn cancellation via CancellationToken, per-session background drain worker, multi-tab QueueUpdated broadcast, /flush, /status, queue persistence across restarts, Discord/Slack/REST queue consumers.
**Requirements**: D-01..D-12 from 36.17.4-CONTEXT.md (no formal REQ-IDs — phase uses D-IDs as the decision spine per 36.17.3 precedent).
**Depends on:** Phase 36.17, Phase 36.17.3 (MessageQueue trait + CommandResult::Queued/PauseQueue/UnpauseQueue variants), Phase 36.17.2.1 (Telegram parity reference)
**Plans:** 6/6 plans complete

Plans:
**Wave 1** *(parallel — Plan 01 owns ironhermes-core::commands::running_agent; Plan 02 owns iron_hermes_ui::protocol; Plan 03 owns iron_hermes_ui::server::state — zero `files_modified` overlap)*

- [x] 36.17.4-01-PLAN.md — Add `pause`/`unpause` to `is_bypass` locked list + extend `is_bypass_locked_list` unit test (RESEARCH Finding 4 fix — closes the D-08 gap so the web ws.rs gate at line 242 lets queue-state-only commands bypass the running-agent guard correctly)
- [x] 36.17.4-02-PLAN.md — Add `ChatStreamEvent::QueueUpdated { depth: u32, paused: bool }` variant to `protocol.rs` + inline `test_queue_updated_json_shape` locking `{"QueueUpdated":{"depth":3,"paused":false}}` external-tagged wire format (D-03 / D-11)
- [x] 36.17.4-03-PLAN.md — Add `queue: Arc<dyn MessageQueue<SessionKey>>` + `queue_paused: Arc<Mutex<HashMap<String, Arc<AtomicBool>>>>` fields to `AppState` + `get_or_create_paused_flag` method mirroring `get_or_create_running_flag` byte-for-byte + three inline unit tests for paused-flag idempotency + session isolation (D-01 / D-01a)

**Wave 2** *(parallel — Plan 04 owns ws.rs; Plan 05 owns hermes_app/mod.rs + hermes_app/app_footer.rs — zero `files_modified` overlap; Plan 04 depends on 01+02+03, Plan 05 depends on 02)*

- [x] 36.17.4-04-PLAN.md — Wire WS recv-loop in `ws.rs`: replace in-flight reject at lines 204-214 with `queue.try_push` + cap-hit fork; add CommandResult::Queued / PauseQueue / UnpauseQueue arms with the canonical Delta + QueueUpdated + Finished + drain rx → socket pattern; extend NewSession arm with D-04 ordering (clear → paused.store(false) → QueueUpdated BEFORE reset_web_session); add early `if def.name == "stop"` intercept with D-04a sequence and zero abort calls (D-05); extend `None =>` branch in arm 2 with the self-drain — emit QueueUpdated to socket BEFORE spawning next run_web_turn (D-02 / D-03 ordering)
- [x] 36.17.4-05-PLAN.md — Wasm UI: HermesApp declares `let mut queue_state = use_signal(|| (0u32, false));` + QueueUpdated match arm `queue_state.set((depth, paused))` + `use_context_provider(|| queue_state)`; AppFooter consumes via `use_context::<Signal<(u32, bool)>>()` and reads into Copy locals before RSX; conditional `QUEUE N` / `QUEUE N (paused)` pill with hide-when-zero discipline placed after the AGENT span in the left run of `.app-footer` (D-03a / D-03b)

**Wave 3** *(depends on all prior plans — every source-text anchor must exist before the grep-based tests pass)*

- [x] 36.17.4-06-PLAN.md — Regression test triplet: `tests/web_queue_drain.rs` (state.rs + ws.rs source anchors + FIFO ordering + cap-at-128 + paused-blocks-drain + paused-flag idempotency direct unit tests on production SessionQueue + Arc<AtomicBool> idiom); `tests/web_queue_controls.rs` (D-04 / D-04a ordering greps + D-05 negative invariant `no handle.abort or cancel_token.cancel in /stop arm` + D-06 cap text + D-08 downstream is_bypass contract + pause/unpause AtomicBool state-machine direct unit test); `tests/web_queue_protocol.rs` (protocol.rs variant + inline shape test name/literal + mod.rs queue_state signal/arm/provider + app_footer.rs pill anchors + user-visible string canaries). D-12 phase gate verified.

### Phase 36.17.3: wire up TUI with gateway queue and slash queue commands (INSERTED)

**Goal:** Extract a shared `MessageQueue<K>` trait into `ironhermes-core` (with `SessionKey` relocated to core + back-compat re-export from gateway, `String` surface at the trait boundary, gateway `SessionQueue` becoming the first concrete implementor with zero behavior change), then wire the ratatui TUI (`crates/ironhermes-cli/src/tui_rata`) as the first non-gateway consumer: `App` owns `Arc<dyn MessageQueue<SessionKey>>` keyed by a fixed `SessionKey { platform: Local, chat_id: "local", user_id: "local" }` (D-03); `/queue <text>` pushes onto the queue and emits inline transcript `Queued: "<text>" (N in queue)` instead of pre-populating the textarea (closes the deferral marker at `commands.rs:1123`); post-turn auto-drain hook in `handle_stream_event::Finished` with paused + in-flight guards (D-04/D-05/T-04); `/pause` toggles `Arc<AtomicBool>` and `/unpause` (NOT `/resume` — avoids registry collision at `registry.rs:88`) explicit-sets to false (D-06 amended); `/new` and `/reset` clear queue + reset paused BEFORE session clear (D-07); `/stop` clears queue then fires `cancel_child.take().cancel()` in that exact order per Pitfall 1 (D-08); status-bar `Queue: N` / `Queue: N (paused)` pill with hide-when-zero discipline read live per frame (D-09); cap=128 → `QueueError::CapacityReached` inline error `Queue is full (128/128). /stop or /flush to drain.` (terminal bell omitted per Resolution 7) (D-10); regression test `tui_queue_regression_d11` covering CONTEXT.md D-11 7-step sequence + D-12 negative-control hook that fails against the pre-fix textarea-prepopulate handler (manual rebase protocol documented in `crates/ironhermes-cli/tests/README.md`). Gateway test suite must stay green throughout (T-03). Out of scope: iron_hermes_ui web wiring (Phase 36.17.4), Discord/Slack/REST consumers, `/status`, `/flush`, dedicated queue panel, queue persistence, sticky-pause/per-turn-pause variants, trait-level drain protocol or observer hooks.
**Requirements**: D-01..D-12 from 36.17.3-CONTEXT.md (D-06 amended during plan-phase to use `/unpause` alias instead of `/resume`); threats T-01..T-04 from plan-phase threat model.
**Depends on:** Phase 36.17
**Plans:** 6/6 plans complete

Plans:
**Wave 1** *(parallel — Plan 01 owns ironhermes-core::queue + ironhermes-core::session + gateway session.rs/session_queue.rs; Plan 02 owns ironhermes-core::commands + downstream defensive match arms — zero file overlap)*

- [x] 36.17.3-01-PLAN.md — Extract `MessageQueue<K: Hash + Eq + Clone + Send + Sync + 'static>` trait into `ironhermes-core::queue` (try_push/pop/len/clear surface; peek omitted per Resolution 3); relocate `SessionKey` + `QueueError` + `MAX_QUEUE_DEPTH`/`WARN_QUEUE_DEPTH` constants into core; re-export `SessionKey` and `QueueError` from `ironhermes-gateway` for back-compat; implement `MessageQueue<SessionKey> for SessionQueue` via `String`→`MessageEvent` adapter (Resolution 5); confirm `cargo test -p ironhermes-gateway` stays green (T-03 gate) (D-01, D-02, T-03)
- [x] 36.17.3-02-PLAN.md — Add `CommandResult::PauseQueue` and `CommandResult::UnpauseQueue` unit variants in `ironhermes-core::commands::mod.rs`; register `/pause` CommandDef with `/unpause` alias (NOT `/resume` — avoids collision at registry.rs:88 per RESEARCH Pitfall 4 / D-06 amended); add defensive `SlashOutcome::Silent` no-op match arms in every exhaustive `match CommandResult` site (TUI `map_core_to_slash_outcome`, gateway handler, classic CLI REPL); smoke-test `cargo test -p ironhermes-core` to confirm no duplicate-name panic at startup (D-06)

**Wave 2** *(parallel — Plan 03 owns tests/ + App::new_test_with_queue; Plan 04 owns App fields + drain hook + status_line + ui.rs — both depend on Plan 01's trait + Plan 02's variants)*

- [x] 36.17.3-03-PLAN.md — Wave 0 test scaffolding: add `App::new_test_with_queue(Arc<dyn MessageQueue<SessionKey>>)` under `#[cfg(feature = "test-support")]` in app.rs mirroring `App::new_test_empty`; create three skeleton test files (`tests/tui_queue_drain.rs`, `tests/tui_queue_controls.rs`, `tests/status_line_queue_pill.rs`) with `#[ignore]`d placeholder stubs; create `tests/README.md` documenting the D-12 manual rebase protocol (D-11, D-12)
- [x] 36.17.3-04-PLAN.md — App wiring + drain + status bar: add `queue: Arc<dyn MessageQueue<SessionKey>>`, `queue_key: SessionKey` (fixed TUI key), `queue_paused: Arc<AtomicBool>` (Arc per Pitfall 6) fields to AppDeps + App + test_deps; add `maybe_drain_queue` method with paused + in-flight guards (Pitfall 3 → T-04 mitigation); hook drain into `StreamEvent::Finished` arm (D-04); set queue_paused=true in `StreamEvent::Error` arm per Resolution 4; explicit `// do NOT drain on cancel` comment in `Cancelled` arm (Pitfall 1); add `queue_depth` + `queue_paused` to `StatusLineState`; render `Queue: {N}` / `Queue: {N} (paused)` pill in `build_pills` with hide-when-zero discipline; wire ui.rs to read `app.queue.len(&app.queue_key)` LIVE per frame to avoid one-tick staleness (Pitfall 5) (D-03, D-04, D-05, D-09, T-04)

**Wave 3** *(Plan 05 ships the slash-command surface against Plan 04's App fields; Plan 06 fills the Plan 03 test bodies and exercises Plan 05's wiring through the App state machine — must run after both)*

- [x] 36.17.3-05-PLAN.md — Slash-command wiring: replace `/queue` arm in commands.rs:978-999 (delete textarea-prepopulate, push via `app.queue.try_push(&app.queue_key, msg)`, emit `Queued: "<text>" (N in queue)` on Ok, emit `Queue is full (128/128). /stop or /flush to drain.` on `CapacityReached` per D-10; bell omitted per Resolution 7); close deferral marker at commands.rs:1123-1129; add `/pause` arm (`fetch_xor(true, SeqCst)` toggle) and `/unpause` arm (`swap(false, SeqCst)` explicit set + "was not paused" no-op message) BEFORE `map_core_to_slash_outcome` fallback; hook `/new` + `/reset` arms to call `app.queue.clear` + `queue_paused.store(false, SeqCst)` BEFORE session-clear path (D-07; T-02); rewrite `/stop` arm to call `app.queue.clear` THEN `app.queue_paused.store(false)` THEN `cancel_child.take().cancel()` in EXACT order per Pitfall 1 then forward to `map_core_to_slash_outcome` for core ProcessRegistry drain (D-08) (D-06, D-07, D-08, D-09, D-10, T-01, T-02)
- [x] 36.17.3-06-PLAN.md — Regression test bodies: replace Plan 03 stubs with real bodies driving App state machine directly. `tui_queue_drain.rs`: `tui_queue_drain_fifo` (5-item FIFO drain via repeated `handle_stream_event(Finished)` cycles), `tui_queue_regression_d11` (full CONTEXT.md D-11 7-step sequence: free-text + 5 queued → assert FIFO order, then 2 queued + pause + unpause + drain, then 3 queued + `/new`-equivalent → assert empty), `tui_queue_regression_negative_control` (D-12 hook asserting `queue.len == 5` after 5 pushes — fails under pre-fix textarea path). `tui_queue_controls.rs`: `tui_queue_pause_unpause`, `tui_queue_new_clears_atomic`, `tui_stop_clears_queue` (witness clear-before-cancel ordering), `tui_queue_cap_hit` (uses `MAX_QUEUE_DEPTH` const + asserts `QueueError::CapacityReached`). `status_line_queue_pill.rs`: pill format + hide-when-zero invariant + inline transcript line format stability guard. Phase gate: full CLI suite green AND gateway suite still green (T-03 re-witness) (D-11, D-12)

### Phase 36.17.1: in-mem FIFO queuing parity of python deque for chat sessions (INSERTED)

**Goal:** Port hermes-agent's per-session `/queue` FIFO mechanism (`gateway/run.py` §2304-2415) into IronHermes so messages arriving while a per-session agent is busy are queued in arrival order and replayed one full agent turn per queued item, with no merging. Ships Telegram-only (D-02): the queue data structure (single `Mutex<HashMap<SessionKey, VecDeque<MessageEvent>>>` on `GatewayRunner`, 128-message per-session cap with drop-newest + ❌ reaction + chat-reply UX on cap-hit, soft warn at 75%), `/queue` slash command (replaces broken stub at `handlers.rs:1607-1621` via new `CommandResult::Queued` variant), busy-agent enqueue (replacing the reject branch at `handler.rs:840-854`), post-turn drain loop (per-chat worker), `/new` + `/reset` clearing hooks (clear BEFORE `store.remove` per Pitfall 5), drain-mode flag (`is_draining: Arc<AtomicBool>` flipped before `self.cancel.cancel()` so the queue keeps accepting late arrivals in-process), and a `#[cfg(test)]`-isolated `SplitSlotQueue` parity mirror with proptest equivalence (1024 cases) against Python's `pending_slot + overflow_list` layout — zero runtime cost. Discord, Slack, web, and `/goal` continuation are out of scope (D-02, D-04, D-05).
**Requirements**: TBD (phase partially anticipates GW-03 per CONTEXT.md but reqs are not pinned)
**Depends on:** Phase 36.17
**Plans:** 5/5 plans complete

Plans:
**Wave 1**

- [x] 36.17.1-01-PLAN.md — SessionQueue type + QueueError + MAX_QUEUE_DEPTH=128 + WARN_QUEUE_DEPTH=96 + `#[cfg(test)] mod parity` SplitSlotQueue mirror + proptest equivalence (1024 cases); add `proptest` dev-dep; declare module in `lib.rs` (D-06..D-11)

**Wave 2** *(depends on Wave 1)*

- [x] 36.17.1-02-PLAN.md — GatewayRunner wiring: `Arc<SessionQueue>` field + 5 public API methods (`try_enqueue`/`dequeue`/`queue_len`/`clear_queue`/`retain_queue`) + thread `Arc<SessionQueue>` into `GatewayMessageHandler` via `build_gateway_handler` (Option-fallback for backward-compat); replace busy-reject at `handler.rs:840-854` with try_push + D-13 cap-hit UX (❌ reaction + chat reply); post-turn drain loop in the per-chat worker calling `run_agent` directly (Pitfall 4) (D-14..D-17)

**Wave 3** *(parallel — 03 owns commands/mod.rs + commands/handlers.rs + handler.rs Queued arm; 04 owns runner.rs is_draining — zero overlap with 03 within runner.rs since 04 touches shutdown sequence + new accessor only)*

- [x] 36.17.1-03-PLAN.md — `/queue` slash command parity: new `CommandResult::Queued { message: String }` variant + rewrite `cmd_queue` (drop the `ctx.agent_loop` gate per Pitfall 3); intercept `CommandResult::Queued` in gateway handler (synthesize `MessageEvent` inheriting platform/chat_id/sender_id from the triggering event, call `session_queue.try_push`, reply with depth-aware "Queued for the next turn." / "({n} queued)" or cap-hit UX); wire `/new` + `/reset` to call `session_queue.clear(&session_key)` BEFORE `session_store.remove(&session_key)` (Pitfall 5); verify `is_bypass("queue") == true`
- [x] 36.17.1-04-PLAN.md — Drain-mode preservation (D-03): add `is_draining: Arc<AtomicBool>` field on `GatewayRunner` + `drain_for_restart()` method that flips the flag BEFORE `self.cancel.cancel()` (atomic source-order, awk-checked); replace shutdown's `self.cancel.cancel()` (runner.rs:~902) with `self.drain_for_restart()`; contract: `try_push` does NOT consult `is_draining` (preserve AND accept new pushes during the in-process drain window); 4 unit tests close T-36.17.1-03

**Wave 4** *(depends on Wave 2 + Wave 3)*

- [x] 36.17.1-05-PLAN.md — Telegram cap-hit UX end-to-end integration tests (busy-enqueue silence, cap-hit ❌+chat-reply with cap held at 128, FIFO post-turn drain "A","B","C" replay with no merging) + UAT runbook `tests/session_queue_telegram_uat.md` (4 scenarios: silent busy enqueue, `/queue` depth-aware reply, cap-hit live verification including Telegram offset-advance no-re-delivery per Pitfall 6, `/new` clears queue); blocking-human checkpoint for live Telegram verification

### Phase 36.17.2: unify session queue — replace UserQueueManager mpsc buffer with SessionQueue (INSERTED)

**Goal:** Make 36.17.1's `SessionQueue` reachable in production Telegram by collapsing `UserQueueManager`'s internal per-chat `mpsc` buffer into `SessionQueue`. UAT of 36.17.1 confirmed that messages 2..N of a per-chat burst sit in `UQM`'s `chat_rx` and never see `agent_running == true` at `handle_with_multimodal` entry — the busy-branch enqueue and post-turn `drain_pending` are dead code on the Telegram path. Locked architecture (option C): `UserQueueManager` keeps its public surface (`dispatch`, transport-layer 👁 reaction emitter, whitelist/`@mention`/multimodal hooks, post-turn `remove` lifecycle) but rips out the per-chat `mpsc` interior; `dispatch` pushes straight into `SessionQueue::try_push` with strict backpressure (full → drop + 429-class bubble to transport). The per-chat worker reads from `SessionQueue` and the old `drain_pending` collapses into the worker's natural pop loop. 👁 reaction is emitted **by the worker** when it pops a message and begins its turn — not when `dispatch` lands it on the queue — so the 👁 reflects actual processing, never buffer occupancy. `/queue` (slash command), `/new`/`/reset` queue-clear ordering (Pitfall 5), `is_draining` flag + `drain_for_restart`, and the cap-hit ❌+chat-reply UX from 36.17.1 are preserved unchanged. Discord/Slack/web/REST gateways re-use the new path automatically (they already flow through `UserQueueManager::dispatch`).
**Requirements**: TBD (closes T-36.17.1 unreachability gap surfaced in 36.17.1-05 UAT — references locked decisions D-01..D-22 from 36.17.2-CONTEXT.md)
**Depends on:** Phase 36.17.1
**Plans:** 5/5 plans complete

Plans:
- [x] 36.17.2-01-PLAN.md — UserQueueManager internal rewrite: replace mpsc::Sender map with SessionKey-keyed worker-presence + Notify map; dispatch returns Result<DispatchOutcome, QueueError>; cap-hit UX (❌+chat reply) migrates into dispatch; rekey by full SessionKey triple; multimodal sidecar (pending_multimodal); with_rate_limit_retry relocated to rate_limiter.rs (D-01..D-03, D-10, D-11, D-13, D-14, D-18, D-19)
- [x] 36.17.2-02-PLAN.md — Per-chat worker rewrite in runner.rs: chat_rx.recv pattern → SessionQueue::pop + Notify::notified select loop; 👁 transport reaction moves into the worker at pop-time (D-08); post-turn drain_pending call REMOVED (D-07); dispatch loop matches on DispatchOutcome (D-04..D-09, D-15, D-16)
- [x] 36.17.2-03-PLAN.md — Integration test tests/uqm_session_queue_unification.rs: 5 same-chat messages → 5 👁 at pop in FIFO order through real handle_with_multimodal; multimodal round-trip test; worker-exit/dispatch race coverage; verify 9 existing tests in session_queue_integration.rs still pass unchanged (D-20, D-21)
- [x] 36.17.2-04-PLAN.md — Telegram live UAT runbook update + blocking-human checkpoint: rewrite session_queue_telegram_uat.md Scenario 1 (👁 at pop time, not dispatch); add Scenario 5 (T-36.17.2-01 worker-exit/dispatch race); add Scenario 6 (T-36.17.2-04 multimodal sidecar lockstep); D-12 deferred footnote; preserve cap-hit + /new + drain-mode scenarios verbatim; phase sign-off gated on user typing "approved" (D-22, T-36.17.2-01..04)
- [x] 36.17.2-05-PLAN.md — Slash-command fast-path (closes second UAT failure): runner.rs dispatch loop branches on event.content.starts_with("/") BEFORE UQM.dispatch and tokio::spawns handler.handle_with_multimodal directly so commands bypass the per-chat worker; sem_dispatch permit acquired in spawn (T-36.17.2-06 storm-bypass mitigation); integration test test_slash_command_bypasses_per_chat_worker + live UAT Scenario 7 (D-23..D-27, T-36.17.2-05/06)

### Phase 36.17.2.1: fix /queue slash-command failing to wake parked worker — regression from Phase 36.17.2's mpsc→Notify worker rewrite; /queue pushes to SessionQueue but does not call notify_one(), so 128/129 messages never reach the LLM (UAT 2026-05-28T15:36-15:38 UTC) (INSERTED)

**Goal:** Restore the /queue slash-command's depth-128 buffering by routing the handler's CoreCommandResult::Queued arm through UQM::dispatch (Option B from RESEARCH.md) so push + notify_one() are atomic. Adds a regression integration test that exercises the EXACT production fast-path invocation (handler.handle_with_multimodal against a real parked worker) so this regression cannot recur silently in CI.
**Requirements**: 36.17.2.1-D-01..D-09 (locked in plan frontmatter); closes the UAT failure 2026-05-28T15:36-15:38 UTC; inherits T-36.17.2-01/02/04 from parent phase and adds T-36.17.2.1-01..06.
**Depends on:** Phase 36.17.2
**Plans:** 2/2 plans complete

Plans:
- [x] 36.17.2.1-01-PLAN.md — Add Option<Arc<UserQueueManager>> field + set_user_queue_manager setter on GatewayMessageHandler; rewrite the CoreCommandResult::Queued arm to delegate to uqm.dispatch(queued_event, None, None).await so push + notify_one() are atomic (Option B from RESEARCH.md); preserve depth-aware reply via session_queue.len() AFTER dispatch; cap-hit UX deduplicated (UQM already fires ❌ + "Queue is full"); legacy direct-try_push fallback preserved when uqm field is None (D-20 contract); reorder runner.rs::run_gateway to construct UQM BEFORE handler Arc-wrap so setter can run on the mutable handler.
- [x] 36.17.2.1-02-PLAN.md — Append regression test test_queue_command_wakes_parked_worker to tests/uqm_session_queue_unification.rs that reuses RecordingFailingAdapter + spawn_test_worker + make_event_full; dispatches 1 free-text via uqm.dispatch to register the worker, polls until the worker drains it and parks at notify.notified(), then dispatches 5 /queue events via handler.handle_with_multimodal directly (the exact production fast-path invocation that exposed the bug); polls the 👀-reaction count on synthesized message_ids q_1..q_5 with a 5-second timeout; asserts 5 reactions in FIFO order + queue drained + send_log grew by baseline+5. Test times out under unfixed code; passes under Plan 01's fix.

### Phase 36.16: Small Model Mode (SMM) architecture port — mirror the smallcode JS reference architecture (System Overview / Component Responsibilities / Layers / Data Flow / Key Abstractions / Entry Points / Architectural Constraints / Anti-Patterns / Error Handling / Cross-Cutting Concerns) into ironhermes Rust; consumes 36.15's per-provider extra_request_options knob as one input; see 36.16-CONTEXT.md (from SmallModelMode_ARCHITECTURE.md) for the reference shape (INSERTED)

**Goal:** [Urgent work - to be planned]
**Requirements**: TBD
**Depends on:** Phase 36
**Plans:** 0 plans

Plans:
- [ ] TBD (run /gsd-plan-phase 36.16 to break down)

### Phase 36.15: Small Model Mode (SMM) — per-provider extra_request_options TOML knob wired through AgentRuntime to ChatRequest.extra so Ollama num_ctx / vLLM top_k / OpenRouter provider.order can be tuned without code changes; closes Ollama exceed_context_size_error fallback path (INSERTED)

**Goal:** Add a per-provider (with optional per-model override) TOML/YAML configuration knob — `extra_request_options` — whose values flow through `AgentRuntime` into `ChatRequest.extra` on every OpenAI-compatible LLM call, so Ollama `num_ctx`, vLLM `top_k`, and OpenRouter `provider.order` (non-Claude routes) can be tuned without code changes. Scope is the knob and its wiring only; the full Small Model Mode architecture port (governor, router, escalation) is Phase 36.16. Closes the Ollama `exceed_context_size_error` fallback path by making `num_ctx` operator-configurable (static knob per D-08; no dynamic retry).
**Requirements**: PROV-11, PROV-12, PROV-13, PROV-14 (added to REQUIREMENTS.md as part of Phase 36.15)
**Depends on:** Phase 36
**Plans:** 6/6 plans complete

Plans:
**Wave 0**

- [x] 36.15-01-PLAN.md — Wave 0 scaffolding: append PROV-11..PROV-14 to REQUIREMENTS.md, finalize ROADMAP.md Phase 36.15 entry, add failing YAML round-trip canary test module to config.rs locking the D-03 shape (Pitfall 1 gate)

**Wave 1** *(parallel — 02 owns config.rs + new config_extras.rs; 03 owns config_schema.rs — zero files_modified overlap)*

- [x] 36.15-02-PLAN.md — Create config_extras.rs (typed OllamaExtraOptions / VllmExtraOptions / OpenRouterExtraOptions / ProviderRouting / ProviderExtraOptions untagged enum / ProviderModelConfig + resolve_extras merge helper); extend ProviderConfig with `extra_request_options` + `models` fields; turn Plan 01 canary GREEN; ADR fallback path documented if untagged-enum mis-deserializes
- [x] 36.15-03-PLAN.md — Append six ConfigField entries to config_schema.rs for canonical extras keys (Ollama num_ctx/num_predict/top_k, vLLM top_k/top_p, OpenRouter provider.order); schema-contains tests; cache_breaking: false invariant (Pitfall 4)

**Wave 2** *(depends on Wave 1)*

- [x] 36.15-04-PLAN.md — Add `resolved_extras` field + `with_resolved_extras` builder to AgentLoop (mirrors `with_provider_name`); substitute `self.resolved_extras.clone()` for literal `None` at `call_llm` (agent_loop.rs:1757) + `call_llm_streaming` (agent_loop.rs:1790); wire AgentRuntime::run_turn to call `ironhermes_core::config_extras::resolve_extras` per-turn (D-10); create `tests/extra_request_options.rs` with three wiremock wire-body tests for Ollama num_ctx + vLLM top_k + OpenRouter provider.order

**Wave 3** *(parallel — 05 appends to tests/extra_request_options.rs; 06 creates tests/invariants_36_15.rs — different files, no overlap)*

- [x] 36.15-05-PLAN.md — Append four invariant tests to tests/extra_request_options.rs: D-09 caller-wins per-key, D-09 stream_options.include_usage floor preserved (client.rs:236), reserved-key collision (named-field-wins, T-36.15-09 mitigation), D-10 mid-session model-switch via resolve_extras with different model_name
- [x] 36.15-06-PLAN.md — Create tests/invariants_36_15.rs with 5 static-grep gates: no literal `None` for extra in agent_loop.rs, `resolved_extras.clone()` count ≥ 2, D-06 `build_openrouter_chat_request_full` still present in any_client.rs, `_extra: Option<HashMap` still present in anthropic_client.rs, client.rs floor markers preserved; full-workspace release build + cargo test gate


### Phase 36.14: SSE stream error fallback gap — detect provider error envelopes inside HTTP 200 SSE bodies and route them through the existing PROV-07 fallback/retry chain (INSERTED)

**Goal:** Close the third major fallback gap class: streaming LLM providers (e.g., OpenRouter) that return HTTP 200 but deliver an error payload as an in-stream SSE `data:` line. Currently `LlmClient::chat_completion_stream` only inspects the HTTP status, so SSE-body errors deserialize-fail silently as `debug!` parse warnings, `call_llm_streaming` returns `Ok` with empty content, and `should_fallback` never fires. Fix adds `StreamEvent::ProviderError(String)` and detection in the stream consumer (when `ChatStreamChunk` deserialization fails AND the data parses as a JSON object with a top-level `error` key), synthesizes a `(NNN Reason)`-formatted error string so `extract_http_status` / `classify_400_subcases` route through the existing classifier, and locks the shape with static-grep invariants in `tests/invariants_36_14.rs`. AnthropicClient streaming path and the `agent_loop.rs` fallback/retry block are NOT modified. Extends Phase 27.1.4.1 (gateway fallback wiring) and 27.1.4.1.1 (transport-error fallback).
**Requirements**: PROV-07 (extension)
**Depends on:** Phase 36
**Plans:** 1/1 plans complete
**Status:** COMPLETE (2026-05-26) — 18 new tests green (7 unit + 7 wiremock integration + 4 static-grep invariants); cargo build --workspace + cargo test -p ironhermes-agent green; atomicity verified (variant + match arm in same commit); see `.planning/phases/36.14-sse-stream-error-fallback-gap/36.14-VERIFICATION.md`.

Plans:
- [x] 36.14-01-PLAN.md — Detect SSE error envelopes in stream consumer, add `StreamEvent::ProviderError` variant, propagate as `Err` in `call_llm_streaming`, add invariants_36_14.rs static-grep regression gates

### Phase 36.13: Plugins & extensions — DECISION: REJECT plugin loader port (lean on skills+MCP+crates per "ironhermes is its own thing" strategic posture). Ship: (1) OpenTelemetry exporter subsuming hermes-agent's observability plugin (Datadog/New Relic via OTLP); (2) ctx.llm + tool_override as direct AgentRuntime primitives (no plugin system); (3) ADR documenting the decision in PROJECT.md / ARCHITECTURE.md; (4) confirm overlap mapping — memory/model/platforms/image-gen/video-gen/kanban/browser already covered by other phases (INSERTED)

**Goal:** Reject porting the hermes-agent plugin loader; instead extend AgentRuntime with the three primitives that have no current substrate (observability export, ctx.llm, tool_override), and ratify the decision via ADR.
**Requirements**: TBD
**Depends on:** Phase 36
**Plans:** 0 plans

Plans:
- [ ] TBD (run /gsd-plan-phase 36.13 to break down)

### Phase 36.12: Packaging & distribution parity — Homebrew tap (macOS native install), Nix flake (reproducible builds, NixOS), Termux/Android support, crates.io publication verification, Windows native install (quick_setup_script.ps1 currently exists — verify status); reach feature parity with hermes-agent's distribution matrix (PyPI/Homebrew/Docker/Nix/Termux/Windows beta) (INSERTED)

**Goal:** [Urgent work - to be planned]
**Requirements**: TBD
**Depends on:** Phase 36
**Plans:** 0 plans

Plans:
- [ ] TBD (run /gsd-plan-phase 36.12 to break down)

### Phase 36.11: Configuration & secrets parity — credential source plugins beyond env/config: macOS Keychain, AWS Secrets Manager, Bitwarden CLI (parity targets from hermes-agent); optional extensions: Linux Secret Service / gnome-keyring, Windows Credential Manager, 1Password CLI. Reduces reliance on plaintext .env files; OS-native credential storage (INSERTED)

**Goal:** [Urgent work - to be planned]
**Requirements**: TBD
**Depends on:** Phase 36
**Plans:** 0 plans

Plans:
- [ ] TBD (run /gsd-plan-phase 36.11 to break down)

### Phase 36.10: Memory & state parity — expose session_search tool over existing ironhermes-state FTS5 schema (infrastructure shipped, just no tool wrapper); optionally evaluate adding managed-memory providers (honcho, mem0, supermemory) alongside current sqlite/grafeo/duckdb backends (INSERTED)

**Goal:** [Urgent work - to be planned]
**Requirements**: TBD
**Depends on:** Phase 36
**Plans:** 0 plans

Plans:
- [ ] TBD (run /gsd-plan-phase 36.10 to break down)

### Phase 36.9: MCP server — expose ironhermes as MCP server for Claude Code / Cursor / external clients. Port hermes-agent's 9-tool surface: conversations_list, conversation_get, messages_read (FTS-backed), attachments_fetch, events_poll/wait, messages_send, permissions_list_open, permissions_respond, channels_list. ironhermes-state FTS5 + HookRegistry already provide the needed substrate. (INSERTED)

**Goal:** [Urgent work - to be planned]
**Requirements**: TBD
**Depends on:** Phase 36
**Plans:** 0 plans

Plans:
- [ ] TBD (run /gsd-plan-phase 36.9 to break down)

### Phase 36.8: ACP adapter — Agent Client Protocol server for Zed / VS Code / JetBrains integration (stdio transport, tool listing + dispatch + streaming, approval-event surface, registry-driven uvx-style install). Single biggest 'cannot switch from hermes-agent' blocker for editor-driven users (INSERTED)

**Goal:** [Urgent work - to be planned]
**Requirements**: TBD
**Depends on:** Phase 36
**Plans:** 0 plans

Plans:
- [ ] TBD (run /gsd-plan-phase 36.8 to break down)

### Phase 36.7: Multi-platform gateway parity — port 19 missing platforms from hermes-agent: WhatsApp, Signal, SMS, Email (IMAP/SMTP), Matrix, Mattermost, MS Teams, iMessage (Bluebubbles), LINE, SimpleX, DingTalk, Feishu, Wecom, WeChat (Weixin), QQ, Yuanbao, generic webhook, HTTP REST API server, Home Assistant trigger (INSERTED)

**Goal:** [Urgent work - to be planned]
**Requirements**: TBD
**Depends on:** Phase 36
**Plans:** 0 plans

Plans:
- [ ] TBD (run /gsd-plan-phase 36.7 to break down)

### Phase 36.7.1: Foundation — generic webhook adapter + HTTP REST API server (unblocks any custom integration via webhook; provides a programmatic surface for headless ironhermes use). Other 17 hermes-agent platforms (WhatsApp/Signal/SMS/Email/Matrix/Mattermost/Teams/iMessage/LINE/SimpleX/DingTalk/Feishu/Wecom/Weixin/QQ/Yuanbao/HomeAssistant) DEFERRED (INSERTED)

**Goal:** [Urgent work - to be planned]
**Requirements**: TBD
**Depends on:** Phase 36.7
**Plans:** 0 plans

Plans:
- [ ] TBD (run /gsd-plan-phase 36.7.1 to break down)

### Phase 36.6: TUI parity & visibility fix — BUG: AI responses still not rendering visibly in ratatui TUI; plus Ink-UX feature port (overlays, pickers, skins, thinking panel, command palette, mode picker, model switcher, OSC8 hyperlinks) (INSERTED)

**Goal:** [Urgent work - to be planned]
**Requirements**: TBD
**Depends on:** Phase 36
**Plans:** 0 plans

Plans:
- [ ] TBD (run /gsd-plan-phase 36.6 to break down)

### Phase 36.6.4: TUI polish — OSC8 clickable hyperlinks, skin engine (dark/light/custom themes), terminal compatibility (iTerm2/Kitty/Ghostty/Windows Terminal) (INSERTED)

**Goal:** [Urgent work - to be planned]
**Requirements**: TBD
**Depends on:** Phase 36.6
**Plans:** 0 plans

Plans:
- [ ] TBD (run /gsd-plan-phase 36.6.4 to break down)

### Phase 36.6.3: Ink-UX port — input UX (command palette / slash menu, model/provider switcher with live handoff, picker components) (INSERTED)

**Goal:** [Urgent work - to be planned]
**Requirements**: TBD
**Depends on:** Phase 36.6
**Plans:** 0 plans

Plans:
- [ ] TBD (run /gsd-plan-phase 36.6.3 to break down)

### Phase 36.6.2: Ink-UX port — thinking panel + overlays (skill hub overlay polish, mode picker overlay, generic overlay framework matching Ink modal pattern) (INSERTED)

**Goal:** [Urgent work - to be planned]
**Requirements**: TBD
**Depends on:** Phase 36.6
**Plans:** 0 plans

Plans:
- [ ] TBD (run /gsd-plan-phase 36.6.2 to break down)

### Phase 36.6.1: BUG FIX: AI response visibility — investigate why streamed/final AI responses don't render visibly in ratatui TUI; verify whether feedback_scroll_width_inner formulas (area.width-2 inner, prefix+body+width-1/width, viewport_content_length) ever landed; ship a regression test (SHIPPED 2026-05-26)

**Goal:** Fix the ratatui TUI auto-scroll undershoot so AI responses always land visible at the true viewport bottom after `StreamEvent::Finished`. Root cause (D-01): `transcript_line_count` uses character-ceiling-divide while ratatui's `Paragraph { wrap: Wrap { trim: false } }` uses word-wrap via `WordWrapper`; the per-line undercount accumulates and `transcript_max_scroll = total - visible` ends up too small (or zero for short responses), leaving the tail of the response — or in extreme cases the whole response — hidden below the viewport. Fix replaces `wrapped_line_count` with a word-wrap simulator using `unicode-width`, repairs both call sites in `transcript_line_count` (i==0 prefix-sharing path + i>0 path), and ships D-02 unit + D-03 integration regression tests.
**Requirements**: D-01, D-02, D-03 (from `.planning/phases/36.6.1-.../36.6.1-CONTEXT.md`)
**Depends on:** Phase 36.6
**Plans:** 2/2 plans complete

**Closeout (2026-05-26):** Verifier passed 7/7 automated must-haves: D-02 module `word_wrap_tests` 5/5 pass, D-03 `auto_scroll_lands_at_true_bottom_after_stream_finished` passes, `unicode-width = "0.2"` direct dep landed, `pub(crate) fn compute_transcript_area` confirmed, both `i==0` and `i>0` call sites in `transcript_line_count` route through `word_wrapped_line_count`, old `fn wrapped_line_count` deleted. Plan 02 auto-fix patched a subtle `flush_word` double-count bug in Plan 01's simulator (caught by the broader D-03 test — exactly the integration coverage's intent) and re-baselined 5 insta snapshots to reflect the now-correct scrollbar appearance. Manual UAT (live TUI reproducing screenshot 1 "Hi!" + screenshot 2 `/usage`) confirmed by user.

Plans:
- [x] 36.6.1-01-PLAN.md — Wave 0: Add `unicode-width` direct dep, replace `wrapped_line_count` with `word_wrapped_line_count` (word-wrap simulator), fix both call sites in `transcript_line_count`, add D-02 unit tests in `app.rs`
- [x] 36.6.1-02-PLAN.md — Wave 1: Promote `compute_transcript_area` to `pub(crate)` in `event_loop.rs`, add D-03 end-to-end auto-scroll integration test in `ui.rs`

### Phase 36.5: Provider parity — OAuth provider (Claude Pro/ChatGPT Pro/SuperGrok OAuth flows), Claude Compliance API integration (enterprise audit export), Cloudflare AI Gateway proxy (unified routing/caching/rate-limiting/analytics) (INSERTED)

**Goal:** [Urgent work - to be planned]
**Requirements**: TBD
**Depends on:** Phase 36
**Plans:** 0 plans

Plans:
- [ ] TBD (run /gsd-plan-phase 36.5 to break down)

### Phase 36.4: Skills library — bundle hermes-agent's 27 built-in + 18 optional skills; install via GitHub, migrate from hermes-agent, or openclaw local install (INSERTED)

**Goal:** [Urgent work - to be planned]
**Requirements**: TBD
**Depends on:** Phase 36
**Plans:** 0 plans

Plans:
- [ ] TBD (run /gsd-plan-phase 36.4 to break down)

### Phase 36.4.3: Openclaw catalog bridge — consume Claude Code openclaw-shaped skill ecosystem via ironhermes-mcp; expose openclaw skills as first-class tools without re-porting (INSERTED)

**Goal:** [Urgent work - to be planned]
**Requirements**: TBD
**Depends on:** Phase 36.4
**Plans:** 0 plans

Plans:
- [ ] TBD (run /gsd-plan-phase 36.4.3 to break down)

### Phase 36.4.2: Hermes-agent skill port — Tier 1 only (github, productivity, devops, software-development, research, email, data-science, mcp); translate Python-handler skills to ironhermes YAML+Markdown bundles or MCP-bridge wrappers (INSERTED)

**Goal:** [Urgent work - to be planned]
**Requirements**: TBD
**Depends on:** Phase 36.4
**Plans:** 0 plans

Plans:
- [ ] TBD (run /gsd-plan-phase 36.4.2 to break down)

### Phase 36.4.1: GitHub tap setup + lock-file seed — point ironhermes-hub at huggingface/skills (or new ironhermes/skills repo); seed Tier-1 skills via SKILL.lock without any code port (INSERTED)

**Goal:** [Urgent work - to be planned]
**Requirements**: TBD
**Depends on:** Phase 36.4
**Plans:** 0 plans

Plans:
- [ ] TBD (run /gsd-plan-phase 36.4.1 to break down)

### Phase 36.3: Tools parity — vision/image/video gen, TTS/STT, computer_use, smart-home, kanban, planning tools (todo/clarify/session_search), first-class send_message, multi-environment exec, browser CDP/dialog (INSERTED)

**Goal:** [Urgent work - to be planned]
**Requirements**: TBD
**Depends on:** Phase 36
**Plans:** 0 plans

Plans:
- [ ] TBD (run /gsd-plan-phase 36.3 to break down)

### Phase 36.3.12: Multi-environment exec — Docker, SSH, Modal, Daytona, Singularity backends (INSERTED)

**Goal:** [Urgent work - to be planned]
**Requirements**: TBD
**Depends on:** Phase 36.3
**Plans:** 0 plans

Plans:
- [ ] TBD (run /gsd-plan-phase 36.3.12 to break down)

### Phase 36.3.11: Web search expansion — Brave, DDG, SearXNG; pluggable web_search_registry (INSERTED)

**Goal:** [Urgent work - to be planned]
**Requirements**: TBD
**Depends on:** Phase 36.3
**Plans:** 0 plans

Plans:
- [ ] TBD (run /gsd-plan-phase 36.3.11 to break down)

### Phase 36.3.10: Browser polish — CDP tool, dialog tool, Camofox-style privacy backend (INSERTED)

**Goal:** [Urgent work - to be planned]
**Requirements**: TBD
**Depends on:** Phase 36.3
**Plans:** 0 plans

Plans:
- [ ] TBD (run /gsd-plan-phase 36.3.10 to break down)

### Phase 36.3.9: Planning tools — todo, session_search (INSERTED)

**Goal:** [Urgent work - to be planned]
**Requirements**: TBD
**Depends on:** Phase 36.3
**Plans:** 0 plans

Plans:
- [ ] TBD (run /gsd-plan-phase 36.3.9 to break down)

### Phase 36.3.8: Messaging & clarification tools — first-class send_message tool, clarify with native buttons (INSERTED)

**Goal:** [Urgent work - to be planned]
**Requirements**: TBD
**Depends on:** Phase 36.3
**Plans:** 0 plans

Plans:
- [ ] TBD (run /gsd-plan-phase 36.3.8 to break down)

### Phase 36.3.7: Kanban / multi-agent board — kanban_* tools (INSERTED)

**Goal:** Ship the IronHermes Kanban v1 kernel — a durable `~/.ironhermes/kanban.db` SQLite board (5 tables, WAL, atomic CAS claim), a gateway-embedded dispatcher with 8-step tick + live-PID detection + respawn-guard + failure circuit-breaker, full-OS-process worker spawn (`ironhermes --profile P --skills kanban-worker chat -q "..."`) with env scrub and 9-env-var contract, a 6-tool LLM surface (`kanban_show/list/complete/block/comment/create`) gated by `HERMES_KANBAN_TASK`, `KANBAN_GUIDANCE` prompt injection, two bundled skills (`kanban-worker` v2.0.0 + `kanban-orchestrator` v3.0.0) synced via `ensure_home_dirs()` + `skills update`, full `ironhermes kanban` CLI + `/kanban` slash command (Universal platform; mid-run bypass), and 10 critical protocol-correctness invariants under automated test. Deferred to 36.3.7.x: heartbeat/link/unblock tools, triage decomposer, multi-board, dashboard plugin, gateway notifier, swarm helper, @mention parser, portable profiles, external CLI lanes.
**Requirements**: D-01..D-41 (CONTEXT.md locked decisions; 41 design decisions across schema/dispatcher/worker-spawn/tools/protocol/skills/workspaces/CLI/gateway/multi-tenant/concurrency)
**Depends on:** Phase 36.3
**Plans:** 9/9 plans complete

Plans:
- [x] 36.3.7-01-PLAN.md — Crate skeleton + types/paths/events/config + `chat -q` flag + invariants test scaffold + sysinfo checkpoint (Wave 0)
- [x] 36.3.7-02-PLAN.md — Schema (5 tables + WAL + migrations) + KanbanStore CRUD + atomic CAS claim (BEGIN IMMEDIATE) + claim_lock/expected_run_id gates + concurrency test (Wave 1)
- [x] 36.3.7-03-PLAN.md — PID liveness + worker env scrub (build_kanban_worker_env) + spawn_worker + 8-step dispatcher tick (detect-crashed → live-PID extension → reclaim → max-runtime → ready-promotion → atomic-claim → respawn-guard → spawn) + failure circuit-breaker (Wave 2)
- [x] 36.3.7-04-PLAN.md — 6 LLM tools (show/list/comment/complete/block/create) with HERMES_KANBAN_TASK gating + protocol-terminator guards (expected_run_id + created_cards) + idempotency_key dedup + workspace validation (Wave 3)
- [x] 36.3.7-05-PLAN.md — KANBAN_GUIDANCE static const + add_skill_overlay hook + register_kanban_tools wiring in CLI Chat handler (Wave 3)
- [x] 36.3.7-06-PLAN.md — Full CLI verb surface (24+ verbs) + KanbanCommands enum + CommandDef registry (Universal platform) + is_bypass extension (D-36 mid-run /kanban) + iron_hermes_ui regression test (Wave 4)
- [x] 36.3.7-07-PLAN.md — Bundle skills/kanban-worker + skills/kanban-orchestrator (byte-for-byte upstream with `hermes -p`→`ironhermes --profile`, `~/.hermes/`→`~/.ironhermes/`) + sync via ensure_home_dirs + skills update + skills reset --restore (Wave 4)
- [x] 36.3.7-08-PLAN.md — Gateway-embedded dispatcher spawn (step 11 in GatewayRunner::start join_set) + HERMES_KANBAN_DISPATCH_IN_GATEWAY=0 override + clean shutdown via CancellationToken (Wave 5)
- [x] 36.3.7-09-PLAN.md — End-to-end lifecycle test (stub spawn_fn) + protocol-violation test + invariants finalization (10 critical + 10 static-grep) + INV-36.3.7.md ledger + docs/kanban/reference.md v1-scope reconciliation + 2 manual checkpoints (live worker spawn + live /kanban bypass) (Wave 6)

### Phase 36.3.7.0: Kanban v1 — UAT-discovered fixes (INSERTED)

**Goal:** Close the three live-runtime bugs surfaced by 36.3.7's deferred UAT-09-A and UAT-09-B (see `.planning/phases/36.3.7-kanban-multi-agent-board-kanban-tools/36.3.7-09-SUMMARY.md` Addendum). All three are wire-up gaps the automated test surface and gsd-verifier missed: receiver-end of `--skills`, missing handler for `/kanban` slash, and off-by-one in failure circuit-breaker.
**Requirements**: BUG-36.3.7-01 (drop `--skills kanban-worker` auto-pass from `worker_spawn.rs:183` — plan 05's env-gated injection already handles tool registration); BUG-36.3.7-02 (add `cmd_kanban` handler in `crates/ironhermes-core/src/commands/handlers.rs` mirroring `cmd_cron` at line 1062, route subverbs to in-process `KanbanStore`, add `"kanban" =>` dispatch arm); BUG-36.3.7-03 (confirm D-12 intent then change `>` to `>=` in dispatcher.rs failure-limit check); re-run UAT-09-A end-to-end (worker reaches `done`) and UAT-09-B (`/kanban list` in interactive `chat` prints CLI table, sub-second, no LLM tokens).
**Depends on:** Phase 36.3.7
**Plans:** 0 plans (run /gsd-plan-phase 36.3.7.0 to break down)

Plans:
- [ ] TBD (run /gsd-plan-phase 36.3.7.0 to break down)

### Phase 36.3.6: Smart home — Home Assistant ha_* suite (INSERTED)

**Goal:** [Urgent work - to be planned]
**Requirements**: TBD
**Depends on:** Phase 36.3
**Plans:** 0 plans

Plans:
- [ ] TBD (run /gsd-plan-phase 36.3.6 to break down)

### Phase 36.3.5: Computer use — desktop control via cua-driver (cross-provider) (INSERTED)

**Goal:** [Urgent work - to be planned]
**Requirements**: TBD
**Depends on:** Phase 36.3
**Plans:** 0 plans

Plans:
- [ ] TBD (run /gsd-plan-phase 36.3.5 to break down)

### Phase 36.3.4: Voice I/O — TTS (Edge/ElevenLabs/OpenAI/MiniMax) + STT (faster-whisper) (INSERTED)

**Goal:** [Urgent work - to be planned]
**Requirements**: TBD
**Depends on:** Phase 36.3
**Plans:** 0 plans

Plans:
- [ ] TBD (run /gsd-plan-phase 36.3.4 to break down)

### Phase 36.3.3: Video generation — unified video_generate (INSERTED)

**Goal:** [Urgent work - to be planned]
**Requirements**: TBD
**Depends on:** Phase 36.3
**Plans:** 0 plans

Plans:
- [ ] TBD (run /gsd-plan-phase 36.3.3 to break down)

### Phase 36.3.2: Image generation — image_generate with Fal/Pixverse registry (INSERTED)

**Goal:** [Urgent work - to be planned]
**Requirements**: TBD
**Depends on:** Phase 36.3
**Plans:** 0 plans

Plans:
- [ ] TBD (run /gsd-plan-phase 36.3.2 to break down)

### Phase 36.3.1: Vision tools — standalone vision_analyze (cross-provider Claude/GPT-4V/Gemini/Grok) (INSERTED)

**Goal:** [Urgent work - to be planned]
**Requirements**: TBD
**Depends on:** Phase 36.3
**Plans:** 0 plans

Plans:
- [ ] TBD (run /gsd-plan-phase 36.3.1 to break down)

### Phase 36.2: Agent loop & core parity — prompt caching, per-provider rate-limit tracking, usage/cost accounting, error classification (SHIPPED 2026-05-26)

**Goal:** Close the four AGENT-LOOP & CORE parity gaps from iron-hermes-planning §2.1 against hermes-agent v0.14.0 as four interlocking sub-systems sharing a typed classification source. (A) Anthropic-native prompt caching via `cache_control` breakpoints with `system_and_3` strategy (PRMT-08/PRMT-09); configurable TTL 5m/1h default 1h; cache_read/cache_creation Usage fields populated end-to-end; cache-break warnings on the existing PRMT-10/11 channel for the 3 known triggers (model swap, memory edit, context-file edit); strict cached/ephemeral layer assertion at assembly time. (B) Per-provider in-memory `RateLimitTracker` in `ironhermes-agent` keyed on (provider, api_key_hash, model); reads anthropic-ratelimit-* and x-ratelimit-* headers (Anthropic uses RFC 3339 reset, OpenRouter/Nous use float-seconds); reactive 429 fallback when headers absent; emits structured `RateLimitEvent` on a Tokio broadcast channel; api_key_hash is SHA-256-truncated-16-bytes via sha2 0.10.9 (BLAKE3 not in Cargo.lock); cleartext keys never serialize anywhere. (C) Usage/cost ledger via static `pricing.toml` + disk cache at `$HERMES_HOME/pricing-cache.json` + `hermes pricing refresh` CLI; schema migration v9 adds 3 cache+cost columns to `sessions` and creates `usage_events` table with per-turn rows + 2 indexes; every LLM call (success or failure) writes one row atomically with the sessions UPDATE inside a single rusqlite transaction; failed calls write `error_kind = Some(ProviderError::variant_name())` with cost=0; subagent rows tagged with subagent session_id; surfaces via `/usage` slash command + TUI status-bar `$X.YYY │ N.NK tok` pill + per-turn `tracing::info!`. (D) Typed `ProviderError` enum (11 variants: RateLimited, Auth, Billing, ContextLength, Server, Transport, SchemaInvalid, ToolError, ModelNotFound, PayloadTooLarge, Unknown) — additive wrap of the existing `classify_llm_error` (now a `From<ProviderError> for (bool, bool)` facade); ports 30 hermes-agent `error_classifier.py` test cases verbatim into Rust as the parity bar; the typed enum is the canonical classification source consumed by sub-system B (`RateLimited{retry_after}` destructure) and sub-system C (`variant_name()` stored in `usage_events.error_kind`). The existing 12+ Rust classifier tests at agent_loop.rs:2585-2789 pass byte-for-byte verbatim. Anthropic billable input cost formula sums (input_tokens + cache_read + cache_creation) per appropriate per-million rate (Anthropic `input_tokens` field is post-last-breakpoint only). All cost arithmetic is i64 micro-USD integers; f64 conversion is gated to (a) the network refresh boundary and (b) the display layer.
**Requirements**: PRMT-08, PRMT-09 (closed by Area A). Coordinates with PRMT-10, PRMT-11, PROV-07, PROV-09, PROV-10 (no closure).
**Depends on:** Phase 36
**Plans:** 11/11 plans complete

**Closeout (2026-05-26):** Code review per `.planning/phases/36.2-.../36.2-REVIEW.md` produced 10 BLOCKER findings (CR-01..CR-10); all closed. Gateway `/usage` end-to-end fixed via three additional commits (state_store read+write wiring + canonical UUID alignment) plus a chat-truncation regression fix from over-eager `with_intercepts` per-turn registration. Operator tooling added: `hermes pricing refresh --source openrouter`, `hermes pricing backfill [--dry-run] [--clean-orphans]`, per-turn disk-cache merge. Workspace release build clean (3m 53s); 3288 tests pass; 0 failures.

Plans:
**Wave 1** *(foundations — no internal deps)*

- [x] 36.2-01-PLAN.md — Extend AnthropicUsage struct (anthropic_client.rs:138) with #[serde(default)] cache_read_input_tokens + cache_creation_input_tokens; populate outer Usage at lines 533-534 + 819-820
- [x] 36.2-02-PLAN.md — Schema migration v9: ALTER sessions ADD cache_read_tokens/cache_creation_tokens/cost_usd_micros + CREATE TABLE usage_events + 2 indexes; UsageEvent struct + insert_usage_event method + extended update_session_stats signature; atomic transaction pattern
- [x] 36.2-03-PLAN.md — ProviderError typed enum (11 variants) + classify_llm_error_typed in new error_classifier.rs; existing helpers stay in agent_loop.rs as pub(crate); 30 Python error_classifier.py test cases ported verbatim; invariants_27_1_4_1_1.rs stays green; invariants_36_2_error_classifier.rs locks the new surface
- [x] 36.2-04-PLAN.md — PricingRegistry mirroring model_metadata.rs pattern + bundled pricing.toml ($5/$25 opus-4-7 new tokenizer, $15/$75 opus-4-20250514 legacy) + PricingCache disk-cache infra at $HERMES_HOME/pricing-cache.json + compute_cost_micros(in_tok, out_tok, cache_read, cache_create) i64 sum

**Wave 2** *(consumes Wave 1)*

- [x] 36.2-05-PLAN.md — Anthropic cache_control markers on AnthropicMessages arm (system_and_3 strategy: 1 system + last 3 messages); PromptCachingConfig with strict CacheTtl enum (5m/1h, default 1h); cached_layers_must_be_stable() assertion (panic debug / warn release); ChatCompletions arm explicitly untouched in wave 1/2
- [x] 36.2-06-PLAN.md — RateLimitTracker module in ironhermes-agent (NOT core; D-RL-04); (provider, api_key_hash, model) three-axis key with SHA-256-truncated-16-bytes hash; Anthropic + x-ratelimit header parsers branch on provider; broadcast::channel(64) RateLimitEvent emitter; severity Critical at remaining=0 AND reset>=60s; static-grep invariants lock no-fallback-import + no-classify-llm-error + no-blake3 + no-cleartext-key
- [x] 36.2-07-PLAN.md — Wire post-LLM-call write site in agent_loop.rs: success path writes usage_events INSERT + sessions UPDATE atomically inside unchecked_transaction with cost via compute_cost_micros; failure path writes row with error_kind = variant_name() + cost=0; tracker.record_headers(hash_api_key(key), ...) on success; tracker.record_429(...) on ProviderError::RateLimited; per-turn tracing::info! line; context_compressor.record_usage_full extends signature
- [x] 36.2-08-PLAN.md — Three cache-break warning methods on PressureTracker (warn_cache_break_model_swap, warn_cache_break_memory_edit, warn_cache_break_context_file_edit) reusing the existing PRMT-10/11 channel; detection sites in agent_loop.rs at /model swap + memory tool write + context-file mtime poll; session-zero suppression + mtime snapshot idempotency
- [x] 36.2-09-PLAN.md — `hermes pricing list` + `hermes pricing refresh [--force]` CLI subcommands mirroring main.rs:169-172 Models pattern; fetch_from_models_dev implementation replacing Plan 04 stub with strict JSON parsing + f64-to-micro-USD conversion gated at network boundary; failure preserves existing cache file; MODELS_DEV_URL hardcoded

**Wave 3** *(UX surfaces; consumes Wave 2)*

- [x] 36.2-10-PLAN.md — `/usage` slash command handler replacing todo_stub at handlers.rs:1533 (flat-flag --today/--provider/--model/--since Nd parsing); StateStore.query_usage_events with rusqlite params![] bindings only (T-36.2-10-INJ); multi-platform via CommandRouter; TUI StatusLineState gains cost_usd_micros + session_total_tokens fields with build_pills cost+token pills; web UI sidebar surface (or documented deferral); manual UAT checkpoint covering 5 verifications
- [x] 36.2-11-PLAN.md — OPTIONAL STRETCH: OpenRouter Claude cache_control on ChatCompletions arm gated by is_openrouter_claude(provider, model) predicate; cut criterion enforced — if envelope diverges from Anthropic's, plan defers to Phase 36.2.1 without affecting Plan 05 PARITY claim


### Phase 36.1: Running-agent guard parity — web UI + TUI (SHIPPED 2026-05-25)

**Goal:** Extend the Phase 36 per-session running-agent guard (`RunningAgentGuard` RAII, `is_bypass`, D-02 rejection message) to the web UI and TUI surfaces, closing the parity gap identified in `36-BACKLOG.md` items 1 and 3. Two tracks: (A) **Web UI** — add per-WebSocket-session `Arc<AtomicBool>` running flag to `iron_hermes_ui` session state, wire `RunningAgentGuard` at `run_web_turn` entry, add slash-command interception before `run_turn` so `/model` and other state-mutating commands are rejected with the D-02 message while an agent turn is active, and `/stop`/`/new` bypass; (B) **TUI** (`tui_rata`) — replace the `pending_rx.is_some()` ad-hoc check at `tui_rata/commands.rs:537` with the same `Arc<AtomicBool>` + RAII pattern, wire guard at TUI turn entry, add command interception with bypass list parity. Extract shared `RunningAgentState` + `RunningAgentGuard` to `ironhermes-core` (or a shared module) so all three surfaces (gateway, web, TUI) use one canonical implementation — mirrors the `MemoryManagerHandle`/`McpReloader` trait patterns from Phases 20/21.2.
**Requirements**: GW-05-WEB (web UI guard parity), GW-05-TUI (TUI guard parity) (phase-local; defined during /gsd:discuss-phase 36.1)
**Depends on:** Phase 36
**Plans:** 4/4 plans complete

**Closeout (2026-05-25):** All 4 plans merged to develop; 82 tests passing across 4 suites; D-01–D-10 all verified in code; all 3 anti-pattern pitfalls mitigated (SeqCst ordering, guard-inside-async, canonical-name bypass); zero regressions — Phase 36 gateway suite still 11/11 green. Shared `RunningAgentGuard` + `is_bypass` + `AGENT_RUNNING_REJECT_MSG` now live in `ironhermes-core::commands::running_agent`; gateway, web UI, and TUI all import from the single canonical module. CLI `main.rs:1707`/`:2024` remains the deferred outlier per CONTEXT.md (intentionally out of scope for this phase). Final commit: `335ca05d`.

Plans:

- [x] 36.1-01-PLAN.md — Extract RunningAgentGuard + is_bypass + AGENT_RUNNING_REJECT_MSG to ironhermes-core::commands::running_agent; update gateway imports + preserve re-export so Phase 36 suite passes unchanged
- [x] 36.1-02-PLAN.md — Web UI: AppState.running_agents per-session map, ws.rs slash interception + plain-text guard, run_web_turn RAII guard, 6 GW-05-WEB integration tests
- [x] 36.1-03-PLAN.md — TUI: App.agent_running persistent field, commands.rs snapshot replacement + bypass check (Pitfall 4), event_loop.rs spawn_turn guard INSIDE tokio::spawn async block (Pitfall 1), 5 GW-05-TUI integration tests
- [x] 36.1-04-PLAN.md — Cross-surface verification: full workspace test + clippy; fill 36.1-VALIDATION.md per-task map; sign off nyquist_compliant true
