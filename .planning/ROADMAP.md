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

### Phase 37: RUSTSEC-2026-0104 reachable panic

**Goal:** Remediate RUSTSEC-2026-0104 (reachable panic in rustls-webpki CRL parsing, DoS, CVSS 7.5) by forcing the patchable 0.103.x chain to 0.103.13 via `[patch.crates-io]`, document the non-patchable serenity 0.102.8 chain as a tracked exemption, and bump the workspace version to 0.2.0.
**Requirements**: SEC-01, SEC-02, SEC-03, SEC-04, SEC-05, VER-01, VER-02, VER-03, VER-04
**Depends on:** Phase 36
**Plans:** 2/2 plans complete

Plans:
**Wave 1**

- [x] 37-01-PLAN.md — RUSTSEC-2026-0104 Chain 2 patch (rustls-webpki =0.103.13) + lockfile regen + build/test verify + Chain 1 documented exemption

**Wave 2** *(blocked on Wave 1 completion)*

- [x] 37-02-PLAN.md — Workspace version bump to 0.2.0 (root + iron_hermes_ui + ironhermes-exec) + CLI --version confirm

---

### Phase 37.1: setup script not working on macos (INSERTED)

**Goal:** Replace the fragmented, broken IronHermes setup scripts with one unified cross-platform bash installer handling the full lifecycle (install/update/reinstall/uninstall), stand up a GitHub Actions release pipeline that produces the prebuilt binaries the installer downloads, and bring `cli-config.yaml.example` + the setup wizard to full `Config` struct parity. Root-fixes the reported macOS failure (wrong repo name `ironhermes`->`iron-hermes`, no release pipeline, dead crates.io fallback).
**Requirements**: REQ-37.1-01, REQ-37.1-02, REQ-37.1-03, REQ-37.1-04, REQ-37.1-05, REQ-37.1-06, REQ-37.1-07, REQ-37.1-08
**Depends on:** Phase 37
**Plans:** 5/6 plans executed

Plans:
**Wave 0/1**
- [x] 37.1-01-PLAN.md -- Wave 0 test scaffolding (config_parity.rs, wizard_coverage.rs, installer_integration.sh; intentionally red)
**Wave 1**
- [x] 37.1-02-PLAN.md -- Unified installer: install|update|reinstall|uninstall verbs, repo-name fix, IRONHERMES_REPO, quarantine strip, cargo-git fallback, existing-install detection; retire redundant scripts
- [x] 37.1-03-PLAN.md -- Config completeness: 9 missing sections + 17-field kanban section in cli-config.yaml.example; config_parity green
- [x] 37.1-04-PLAN.md -- GitHub Actions release.yml: 5-target matrix (macOS arm64/x64, Linux x86_64/aarch64, Windows x86_64), ad-hoc sign + notarization slot-in (has human-verify checkpoint)
**Wave 2**
- [ ] 37.1-05-PLAN.md -- Setup wizard: every section accounted for (13 commented-default blocks + 17-field kanban), additive merge never clobbers SET values; wizard_coverage green
- [x] 37.1-06-PLAN.md -- Minimal Windows install.ps1 (install verb only; update/reinstall/uninstall deferred per D-12)

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

### Phase 36.17.7: Gateway + web runtime TTS wiring — activate `text_to_speech` / `send_audio` for live sessions (INSERTED, NEW)

**Goal:** Close the runtime-side gap that 36.17.5 deferred and 36.17.6 documented: `register_tts_tools` is guarded by `if let Some(ref session_key) = input.session_key` in `crates/ironhermes-agent/src/app_runtime_factory.rs:97-103`, and `AgentRuntime::from_config` (called by `run_gateway` and by the iron_hermes_ui web server) currently hard-codes `session_key: None, telegram_adapter: None` at `crates/ironhermes-agent/src/agent_runtime.rs:215-217` ("Phase 36.17.5 D-15: per-turn threading deferred to a follow-up phase"). As a result, live agent sessions on Telegram, Discord, Slack, and the iron_hermes_ui web surface have `text_to_speech` + `send_audio` REGISTERED in `ironhermes-tools` but NEVER exposed to the LLM tool-schema list, so the agent silently falls back to skills like `hyperframes-media` or `say`. This phase threads a real `SessionKey` (and the right `AudioDispatcher` adapter per platform) into the per-session bundle so the two TTS tools actually appear in the agent's tool surface on every supported platform.

**Surfaces in scope:** (a) **gateway** path `run_gateway` → `AgentRuntime::from_config` (covers Telegram via existing `impl AudioDispatcher for TelegramAdapter`, Discord + Slack deferred-or-decision below); (b) **iron_hermes_ui web** path `crates/iron_hermes_ui/src/server/...` → whatever runtime constructor it uses (likely `AgentRuntime::from_config` too, or its own ws-handler-scoped factory) — needs a new `WebAudioDispatcher` impl that pipes the produced MP3 file to the browser (WS binary frame / blob URL via existing chat-event channel, NOT a server-side `rodio` playback).

**Out of scope:** new TTS providers, streaming/chunked synthesis, push-to-talk / STT, voice-mode (auto-speak Path A). All four were deferred from 36.17.5 and stay deferred.

**Sketch of locked decisions to confirm in `/gsd:discuss-phase`:**

- **D-01 — where to thread session_key:** gateway handler builds a per-turn `AppRuntimeFactoryInput` with `session_key: Some(SessionKey { platform: Telegram, chat_id, user_id })` and calls `build_app_runtime_bundle` per-session, OR keeps the singleton `AgentRuntime` and adds a registry-mutator path. Default: per-turn bundle (matches 36.17.5 D-15 "tools are tied to the agent loop, not the runtime").
- **D-02 — WebAudioDispatcher transport:** browser audio is delivered as a binary WS frame on the existing `ChatStreamEvent` channel (new typed variant `ChatStreamEvent::AudioOut { mime, bytes }`), with the wasm UI calling `HTMLAudioElement.play()` from a `Blob` URL. Reject the alternatives of (a) writing to `~/.ironhermes/audio_cache` and serving via a new `/audio/:id` HTTP route (cross-origin + cleanup pain), (b) base64-encoding inside the existing JSON event stream (3× bandwidth, blocks streaming). Confirm during discuss.
- **D-03 — Discord/Slack adapters:** Discord supports voice via gateway voice channels (large surface — DEFER); Slack supports `files.upload` audio (small surface — could ship). Default: ship Telegram + Web in this phase; queue Discord + Slack for a separate phase. (Mirrors the 36.17.5 deferral pattern.)
- **D-04 — Per-session SessionKey lifetime:** built fresh at the start of every `run_turn` (gateway handler arm + ws handler arm), threaded through `AppRuntimeFactoryInput` for that turn only; old bundle is dropped when the turn completes. Rationale: keeps the audio dispatcher Arc-clone count bounded, matches the existing `RegistryToolsetSession` per-session lifetime in `run_gateway`.
- **D-05 — Invariant test:** add `crates/ironhermes-agent/tests/invariants_36_17_7.rs` that asserts `from_config` (or its replacement constructor) passes `session_key: Some(...)` on the production code path — flipping the negation of the 36.17.5 deferral guard.

**Requirements (acceptance items, locked at discuss-phase):**

- **A1** — On a live Telegram session, `text_to_speech` + `send_audio` appear in the LLM's tool-schema list for every turn (verifiable by tracing the system message or by an integration test that captures the request payload).
- **A2** — On a live iron_hermes_ui web session, the same two tools appear in the LLM tool-schema list for every turn.
- **A3** — On Telegram, the agent invoking `text_to_speech` + `send_audio` produces an audible voice message in the chat (operator UAT, mirrors 36.17.5 Gate 5).
- **A4** — On the web UI, the agent invoking `text_to_speech` + `send_audio` produces playback in the browser (operator UAT — clicking the audio control plays the synthesized audio).
- **A5** — `cargo test -p ironhermes-agent` includes a new invariant locking the `session_key: Some(...)` wiring (D-05).
- **A6** — No regression in 36.17.5 CLI path (`hermes tts test/play`) or 36.17.6 CLI inspection path (`hermes toolset list/show voice` still shows `2/2 ✓`).
- **A7** — `voice` row in `toolset list` flips from `disabled` to `enabled` for sessions where the agent runtime registered the tools (or document why the slug-level enablement is decoupled from the per-session registration and add a status banner to inspection output).

**Depends on:** Phase 36.17.5 (D-15 deferral), Phase 36.17.6 (CLI inspection scaffolding to verify against), Phase 36.17.4 (web ws ChatStreamEvent variant pattern for D-02).
**Plans:** TBD (sketch — to be detailed by `/gsd:discuss-phase 36.17.7` then `/gsd:plan-phase 36.17.7`).

### Phase 36.17.6: Toolset CLI TTS wiring (INSERTED)

**Goal:** Close phase 36.17.5 Plan 04's blocked UAT Gates 4+5 by wiring the `voice` toolset into the CLI inspection path. Adds `"voice"` to `KNOWN_TOOLSETS` + `toolset_members_map` and a `register_tts_for_inspection` helper called from both `cmd_toolset_list` and `cmd_toolset_show` (D-01/D-02). Re-runs the full 5-gate UAT (D-04) and secures operator approval for Gates 4+5 (D-05). Finally flips `36.17.5-VALIDATION.md` (`nyquist_compliant: true`, `wave_0_complete: true`, `status: complete`) and ROADMAP Plan 04 line to `[x]`, closing 36.17.5 formally.
**Requirements**: A1..A8 + REGR-1 from 36.17.6-CONTEXT.md `<acceptance>` block (CONTEXT-locked acceptance items act as REQ-IDs per the 36.17.x precedent). D-01..D-05 locked decisions.
**Depends on:** Phase 36.17.5 (Plan 04 PARTIAL)
**Plans:** 3/3 plans complete

Plans:
**Wave 1**

- [x] 36.17.6-01-PLAN.md — Add `"voice"` to `KNOWN_TOOLSETS` + `toolset_members_map`, add `register_tts_for_inspection` helper, call from both `cmd_toolset_list` and `cmd_toolset_show`, add `toolset_members_map_voice_entry` test, update `browser_in_known_set` length 8→9. Single-file change to `crates/ironhermes-cli/src/toolset_cmd.rs`. 3 tasks. D-01/D-02/D-03. A1/A2/REGR-1.

**Wave 2** *(depends on Wave 1)*

- [x] 36.17.6-02-PLAN.md — Release-build the binary, pre-flight `voice` row visibility, re-run `bash scripts/uat/phase-36.17.5-tts.sh` (Gates 1-3 automated regression check; Gates 4-5 operator-approved via blocking-human checkpoints). 3 tasks (1 auto + 2 BLOCKING human-verify). D-04/D-05. A3/A4/A5.

**Wave 3** *(depends on Wave 2)*

- [x] 36.17.6-03-PLAN.md — Flip `36.17.5-VALIDATION.md` frontmatter (`status: complete`, `nyquist_compliant: true`, `wave_0_complete: true`), flip ROADMAP Plan 04 line `[ ]` → `[x]`, run `gsd-verifier` over combined 36.17.5 + 36.17.6 surface. 3 tasks. D-05. A6/A7/A8.

### Phase 36.17.5: integrate TTS functions (INSERTED)

**Goal:** Port hermes-agent text-to-speech into IronHermes as a faithful concept-level port of Python TTSProvider ABC + _BUILTIN_NAMES invariant. Ships a TtsProvider trait + TtsRegistry in ironhermes-core; two built-in provider impls in ironhermes-tools (Edge TTS — free default, no API key; ElevenLabs — premium ELEVENLABS_API_KEY); single LLM-callable text_to_speech tool that synthesizes to a file and returns the path; companion send_audio tool that dispatches the produced file via the current SessionKey platform (Local → rodio playback; Telegram → send_voice/send_audio with optional ffmpeg MP3→Opus conversion). Telegram delivery wired this phase; Discord and iron_hermes_ui web deferred. STT, push-to-talk, streaming TTS, and auto-speak (Path A) all out of scope.
**Requirements**: D-01..D-16 (CONTEXT-locked decisions act as REQ-IDs per the 36.17.x precedent; no formal REQ-NN tags in REQUIREMENTS.md for this phase). Phase-local tests TTS-01..TTS-10 in VALIDATION.md.
**Depends on:** Phase 36.17
**Plans:** 3/4 plans executed

Plans:
**Wave 1**

- [x] 36.17.5-01-PLAN.md — Core trait + registry + config + constants + workspace deps (msedge-tts + rodio) + Wave-0 test scaffolds. 7 tasks (2 BLOCKING package-legitimacy human-verify gates + 5 auto). D-08/D-09/D-10/D-11/D-12. TTS-01/02/07/08.

**Wave 2** *(depends on Wave 1)*

- [x] 36.17.5-02-PLAN.md — Provider impls (EdgeProvider via msedge-tts, ElevenLabsProvider via reqwest) + ffmpeg OnceLock probe + build_tts_registry factory. 3 tasks. D-03/D-04. TTS-03/04/09. T-text-length + T-api-key-leak.

**Wave 3** *(depends on Wave 2)*

- [x] 36.17.5-03-PLAN.md — TextToSpeechTool + SendAudioTool + AudioDispatcher trait + register_tts_tools + AppRuntimeFactoryInput extension + per-session wiring + TelegramAdapter::impl AudioDispatcher. 4 tasks. D-05/D-06/D-07/D-13/D-14/D-15/D-16. TTS-05/06. **T-output-path BLOCKING mitigation owned here.**

**Wave 4** *(depends on Wave 3)*

- [x] 36.17.5-04-PLAN.md — `hermes tts test/play` CLI subcommands + 5-gate UAT script + un-ignore TTS-10 live-network test + operator UAT (2 BLOCKING human-verify gates: tool registry exposure + Platform::Local audible playback). 5 tasks. D-01/D-02. TTS-10.

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

### Phase 36.17.2.2: IronHermes Telegram client delivers streaming final media messages (INSERTED)

**Goal:** Bring IronHermes Telegram delivery to parity with the hermes-agent contract in docs/telegram-features.txt — final text rendered as MarkdownV2 with smart escape (D-01/D-04 fence + inline-code + link-URL state machine), streaming intermediate edits stay plain text per inherited 36.17.2 D-03, and <MEDIA: path|url> tags extracted from agent output dispatch as native Telegram attachments (5 types: photo/voice/audio/video/document) via a new MediaSender trait. Media dispatch runs inline in run_agent AFTER the consumer+typing await barriers and BEFORE match agent_result (CORRECTED D-19 anchor per RESEARCH Pitfall 6) so the existing per-chat worker pop-loop and cap-hit UX from 36.17.2 are preserved.
**Requirements**: D-01..D-20 (locked in CONTEXT.md 2026-06-03; cited as D-NN in each plan frontmatter); inherits T-36.17.2-01..06 from parent phase; adds T-INPUT-MEDIA-PATH/-URL/T-MD-INJECTION/T-LOG-LEAK threat-model rows.
**Depends on:** Phase 36.17.2
**Plans:** 7/7 plans complete

Plans:

- [x] 36.17.2.2-01-PLAN.md — markdown_v2.rs TDD: pub fn escape_markdown_v2 + pub fn escape_outside_code_blocks (D-04 smart escape with fence/inline-code/link-URL state machine; Pitfall 1 + Pitfall 5 mitigations); 13+ golden tests written RED first; pub mod wired into lib.rs.
- [x] 36.17.2.2-02-PLAN.md — media_tag.rs TDD: pub struct MediaTagExtractor mirroring StreamingContextScrubber partial-prefix buffering (D-05/D-08) + 3 extra state booleans for D-09 fence/inline-code skip + Pitfall 5 escaped-backtick respect; MediaSource/MediaKind/MediaRef types; 19+ tests covering D-05/D-06/D-08/D-09; pub mod wired into lib.rs.
- [x] 36.17.2.2-03-PLAN.md — MediaSender trait declaration in adapter.rs per D-18 (6 async methods: 5 per-type + send_media dispatcher with default body) with D-17 no-caption divergence doc-comment + Arc dyn-dispatch upcast warning; teach the <MEDIA: ...> convention to the model in prompt_builder.rs Telegram branch (RESEARCH Open Q1 / Assumption A11 mitigation — without this the phase ships dead code).
- [x] 36.17.2.2-04-PLAN.md — telegram.rs structural reconciliation: send_file_multipart -> Result<MessageResponse> (Pitfall 4); add send_audio per D-13; replace edit_message_markdown -> edit_message_markdown_v2 across PlatformAdapter trait + ripple to TelegramAdapter + MockAdapter + RecordingFailingAdapter (RESEARCH FLAGGED RISK option a full replacement); D-02 single-retry-as-plain-text fallback in TelegramAdapter; impl MediaSender for TelegramAdapter with D-12 URL passthrough + D-15 size pre-check + T-INPUT-MEDIA-PATH path canonicalization + T-INPUT-MEDIA-URL scheme gate + T-LOG-LEAK filename-only logging.
- [x] 36.17.2.2-05-PLAN.md — stream_consumer.rs final-edit pipeline: add PlatformAdapter::send_message_markdown_v2 trait method + impls (TelegramAdapter with D-02 fallback; MockAdapter + RecordingFailingAdapter as record-only); apply escape_outside_code_blocks at the line-100 final-edit call site + the 2 overflow-chunk send sites per CONTEXT D-Discretion (every overflow chunk renders consistently as MarkdownV2); D-03 intermediate-edit branch + cursor strip path explicitly preserved.
- [x] 36.17.2.2-06-PLAN.md — handler.rs D-19 wire: media_sender Option<Arc<dyn MediaSender>> field + set_media_sender setter; MediaTagExtractor wired alongside StreamingContextScrubber via chained stream_callback (extractor.feed(&scrubber.feed(delta)) per Open Q5 / Assumption A10); D-19 dispatch loop inserted at the CORRECTED anchor (between consumer_handle.await.ok() at :1439 + typing_handle.await.ok() at :1443 AND match agent_result at :1445); D-10 combined-reinsert in one edit per turn; runner.rs Telegram start path clone-casts adapter to Arc<dyn MediaSender> (NO trait upcasting per Assumption A7); new integration test tests/telegram_media_delivery.rs with RecordingMediaAdapter succeeding-adapter fixture covering D-07/D-09/D-10/D-11/D-15 (7+ tests).
- [x] 36.17.2.2-07-PLAN.md — Telegram live UAT runbook crates/ironhermes-gateway/tests/telegram_media_uat.md (D-20): 9 scenarios per CONTEXT (text-only MarkdownV2 / single photo / voice .ogg / audio .mp3 / multi-tag / URL form / missing path / parse error / fence pass-through); blocking-human checkpoint gated on operator approved reply per inherited 36.17.2 D-22 protocol.

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
**Plans:** 5/5 plans complete

Plans:

- [x] 36.3.7.0-01-PLAN.md — Drop `--skills` argv from worker_spawn.rs; add HERMES_KANBAN_TASK_SKILLS env carrier; receiver-end test (Wave 1)
- [x] 36.3.7.0-02-PLAN.md — Add cmd_kanban handler + dispatch arm + KanbanStoreReader trait + KanbanStoreReaderImpl + build_cmd_ctx wiring + receiver dispatch-chain test (Wave 1)
- [x] 36.3.7.0-03-PLAN.md — D-12 determination commit + hook apply_circuit_breaker into detect_crashed_workers path + 2 receiver-end tests on the crashed path (Wave 1)
- [x] 36.3.7.0-04-PLAN.md — Re-run UAT-09-A + UAT-09-B against post-fix binary; capture evidence; write phase-close SUMMARY with Meta-learning receiver-end-gap rule (Wave 2, human-verify checkpoint)
- [x] 36.3.7.0-05-PLAN.md — UAT-discovered inline: preflight gate excludes chat -q at both run_preflight and is_interactive_repl sites + 3 static-grep regression tests (BUG-36.3.7-04)

### Phase 36.3.7.1: Kanban dispatcher — extend circuit breaker to remaining failure paths (INSERTED)

**Goal:** Close the two structurally-identical receiver-end gaps that Phase 36.3.7.0 Plan 03 explicitly punted: `reclaim_stale_claims` and `enforce_max_runtime` in `crates/ironhermes-kanban/src/dispatcher.rs` both bump `consecutive_failures` but never invoke `apply_circuit_breaker`. Same pattern as BUG-36.3.7-03 on a different code path; the determination doc in `36.3.7.0-03-D12-DETERMINATION.md` already named these as 36.3.7.1 candidates. Fix is mechanical (call the breaker after the bump, mirroring the lines 305 and 899 patterns from 36.3.7.0-03) plus receiver-end tests covering both new paths × both sides of the failure_limit bound.
**Requirements**: BUG-36.3.7.1-01 (wire `apply_circuit_breaker` into `reclaim_stale_claims` path around dispatcher.rs ~line 458 — match the call shape used at lines 305/899); BUG-36.3.7.1-02 (wire `apply_circuit_breaker` into `enforce_max_runtime` path around dispatcher.rs ~line 553); BUG-36.3.7.1-03 (4 receiver-end tests in `dispatcher_logic.rs` covering both new paths × `consecutive_failures` at `failure_limit` and below `failure_limit - 1`, mirroring the test shapes 36.3.7.0-03 used).
**Depends on:** Phase 36.3.7.0
**Plans:** 2 plans

Plans:

- [ ] 36.3.7.1-01-PLAN.md — Wire apply_circuit_breaker into reclaim_stale_claims path + 2 receiver-end tests (Wave 1)
- [ ] 36.3.7.1-02-PLAN.md — Wire apply_circuit_breaker into enforce_max_runtime path + 2 receiver-end tests (Wave 1)

### Phase 36.3.7.2: Tool-schema compatibility — drop top-level oneOf from delegate_task (INSERTED)

**Goal:** Close the out-of-scope blocker discovered during Phase 36.3.7.0 UAT-09-A re-run #5 (see `.planning/phases/36.3.7.0-kanban-v1-uat-discovered-fixes-inserted/36.3.7.0-04-UAT-EVIDENCE.md` section "Discovered: Bug #5"). `crates/ironhermes-tools/src/delegate_task.rs:735` uses top-level `oneOf` to enforce mutual exclusion of `task` vs `tasks` at the JSON Schema level. Anthropic's tool API rejects top-level `oneOf` / `allOf` / `anyOf` in tool `input_schema`, so EVERY worker subprocess routed through Anthropic-via-OpenRouter crashes at the first LLM call with `400: input_schema does not support oneOf, allOf, or anyOf at the top level`. Comments at delegate_task.rs:1221-1239 already note that runtime validation in `execute()` enforces mutual exclusion — the schema-level `oneOf` is redundant safety that can be removed without losing runtime correctness. This phase belongs to `ironhermes-tools` (Phase 21.7-class infrastructure), NOT kanban — it predates 36.3.7 and is exposed by the kanban worker path only because that path is the first end-to-end exerciser of `delegate_task` via Anthropic. HIGH severity (blocks ALL kanban workers via Anthropic-via-OpenRouter routing).
**Requirements**: BUG-IRONHERMES-TOOLS-SCHEMA-COMPAT-01 (drop top-level `oneOf` from `delegate_task` input_schema at delegate_task.rs:735; preserve runtime validation in `execute()` per existing comments at lines 1221-1239; update unit tests at lines 1230-1239 that currently assert on the `oneOf` shape); BUG-IRONHERMES-TOOLS-SCHEMA-COMPAT-02 (audit all other tools in `crates/ironhermes-tools/src/` for top-level `oneOf` / `allOf` / `anyOf` — at minimum a static-grep pass + cite findings in SUMMARY; expand fix surface only if grep returns more hits); BUG-IRONHERMES-TOOLS-SCHEMA-COMPAT-03 (receiver-end test: synthesize the worker's tool registry, render each tool's `input_schema`, assert no top-level `oneOf` / `allOf` / `anyOf` — locks the regression for ALL future tool additions, not just `delegate_task`).
**Depends on:** Phase 36.3.7
**Plans:** 2/2 plans complete

Plans:

- [x] 36.3.7.2-01-PLAN.md — Drop top-level oneOf from delegate_task input_schema, rewrite mutex prose, invert existing test, audit ironhermes-tools/src/ for other hits (BUG-COMPAT-01 + 02)
- [x] 36.3.7.2-02-PLAN.md — System-level receiver-end test asserting no tool's input_schema has a top-level boolean combinator (BUG-COMPAT-03)

### Phase 36.3.7.3: CLI CFG-03 doc-comment scan-window regression (INSERTED)

**Goal:** Close the regression flagged by the Phase 36.3.7.2 verifier (see `.planning/phases/36.3.7.2-tool-schema-compatibility-drop-top-level-oneof-from-delegate/36.3.7.2-VERIFICATION.md` — "Forward-flagged" section). `crates/ironhermes-cli/tests/invariants_26_4_1_cfg_03.rs::phase_amendment_doc_comment_present` is failing because Phase 36.3.7.0 commit `c453411f` rewrote the comment block above the `run_preflight` gate in `crates/ironhermes-cli/src/main.rs`, pushing the original CFG-03 amendment doc-comment outside the test's 1500-char scan window. Test is a pre-existing static-grep regression-lock from Phase 26.4.1; the comment it scans for still exists in the file but is now beyond the position the test reads. Fix is mechanical: either (a) re-add the CFG-03 doc-comment near the run_preflight gate so it falls inside the scan window, OR (b) widen the test's scan window so it finds the comment at its current position — both options preserve the original CFG-03 intent. CONTEXT must lock the choice + add a receiver-end test that asserts the CFG-03 amendment doc-comment is present REGARDLESS of unrelated comment rewrites (search anchored on the BUG-CFG-03 marker, not position). MEDIUM severity (no runtime impact, but blocks `cargo test -p ironhermes-cli` from a green run).
**Requirements**: BUG-CLI-CFG-03-DOC-COMMENT-01 (restore the CFG-03 amendment doc-comment so `tests/invariants_26_4_1_cfg_03.rs::phase_amendment_doc_comment_present` passes again — choose between re-adding the comment near `run_preflight` OR widening the test's scan window in CONTEXT planning); BUG-CLI-CFG-03-DOC-COMMENT-02 (harden the receiver: rewrite the test to anchor on the `BUG-CFG-03` marker/grep instead of a 1500-char window so future unrelated comment rewrites don't trigger false regressions).
**Depends on:** Phase 36.3.7.0
**Plans:** 1/1 plans complete

Plans:

- [x] 36.3.7.3-PLAN.md — Restore CFG-03 marker line + harden test to anchor on async fn main scope (BUG-01 + BUG-02)

### Phase 36.3.7.4: Dispatcher events parity — emit Reclaimed event in reclaim_stale_claims (INSERTED)

**Goal:** Close the latent bilateral doc/impl gap flagged by Phase 36.3.7.1 Plan 01 SUMMARY + verifier forward note FN-1: `reclaim_stale_claims` doc-comment at `crates/ironhermes-kanban/src/dispatcher.rs:19` promises "append `reclaimed` event" but the implementation only emits `tracing::info!(event = "reclaimed", ...)` — there is no `store.append_event(KanbanEventKind::Reclaimed, ...)` call. `KanbanEventKind::Reclaimed` is already a defined variant in `events.rs:48` and is correctly emitted by `store.rs:717` (a sibling path); only the dispatcher's runtime reclaim path is missing the event-row append. Result: downstream consumers (the upcoming gateway notifier in Phase 36.3.7.5, any future event-replay test, any audit query of `task_events`) cannot see reclaim events that originated from the TTL-expiry path. LOW severity (no runtime correctness impact today) but a load-bearing fix for the gateway-notifier work that follows. Producer-only fix; consumer is already in place. Single-plan, orchestrator-inline (same playbook as Phase 36.3.7.3).
**Requirements**: BUG-36.3.7.4-01 (insert `store.append_event(&task.id, run.as_ref().map(|r| r.id.as_str()), KanbanEventKind::Reclaimed, Some(&payload))` into `reclaim_stale_claims` AFTER the `tracing::info!` at dispatcher.rs:437-441 and BEFORE the run-close-out block at ~line 444; payload should carry pid + reason="ttl_expired" + stale_lock per the existing tracing fields); BUG-36.3.7.4-02 (1 receiver-end test in `crates/ironhermes-kanban/tests/dispatcher_logic.rs` asserting that after one tick of `reclaim_stale_claims` on a stale-claim task, the `task_events` table contains exactly one row with `kind='reclaimed'` and the payload contains the expected fields).
**Depends on:** Phase 36.3.7.1
**Plans:** 0 plans (orchestrator-inline at execution)

Plans:

- [x] 36.3.7.4-PLAN.md — Single-plan inline execution; Task 1 verification revealed `cas::release_claim` already emits the `reclaimed` event row via direct SQL INSERT (cas.rs:136-140), so BUG-36.3.7.4-01 was a NO-OP per CONTEXT's conditional gate. BUG-36.3.7.4-02 receiver test shipped as the regression lock. All 7 gates PASS. Commit `1161fc7d`. See SUMMARY.md + VERIFY.md.

### Phase 36.3.7.5: Gateway notifier — auto-subscribe + polling loop + notify-{subscribe,list,unsubscribe} verbs (INSERTED)

**Goal:** Ship the v1-NOTE-deferred gateway notifier as described in `docs/kanban/reference.md` §703 + §759-773. When you run `/kanban create "…"` from the gateway (Telegram / Discord / Slack), the originating chat is automatically subscribed to the new task; a background polling loop in `GatewayRunner` reads `task_events` every few seconds and delivers ONE message per terminal event (`completed`, `blocked`, `gave_up`, `crashed`, `timed_out`) to the subscribed chat. Subscriptions are persistent (a new `kanban_subscriptions` table) and auto-remove themselves on terminal events. Explicit CLI surface (`hermes kanban notify-subscribe/list/unsubscribe`) lets non-gateway callers (cron jobs, scripts, operators) manage subscriptions out-of-band. `--json` callers of `/kanban create` skip auto-subscribe per reference.md §718. The `notification_sources` config key (already RESERVED in `crates/ironhermes-kanban/src/config.rs:63`) gates the polling loop's startup so installs without a gateway never start the poller. HIGH operator value (closes the "tell me when my task is done" loop end-to-end). MEDIUM scope — schema add (1 new table) + 4 new code surfaces (notifier loop + CLI verbs + handler-side dispatch + auto-subscribe hook on `kanban create`). Send paths exist already (`crates/ironhermes-gateway/src/{telegram,discord,slack}.rs::send_message`). Multi-plan via gsd-planner.
**Requirements**: BUG-36.3.7.5-01 (`kanban_subscriptions` table schema migration — task_id, platform, chat_id, thread_id, created_at, source enum {auto, explicit}; indexed by task_id + by (platform, chat_id, thread_id)); BUG-36.3.7.5-02 (`store.append_subscription` / `list_subscriptions_for_task` / `list_subscriptions_for_chat` / `remove_subscription` APIs + auto-remove on terminal-event delivery); BUG-36.3.7.5-03 (`run_notifier_loop(ctx, cancel_token)` in a new `crates/ironhermes-kanban/src/notifier.rs` — every N seconds (default 3s, configurable via `kanban.notifier_poll_seconds`), poll for terminal events on subscribed tasks, send via the gateway's adapter trait, mark each event as delivered to avoid double-send); BUG-36.3.7.5-04 (gateway runner spawns the notifier loop into join_set at `crates/ironhermes-gateway/src/runner.rs:~1278` mirroring the kanban dispatcher spawn pattern, gated on `notification_sources` config + on at least one platform being enabled); BUG-36.3.7.5-05 (3 new CLI verbs: `kanban notify-subscribe <task-id> --platform <P> --chat-id <C> [--thread-id <T>]`, `kanban notify-list [<task-id>] [--json]`, `kanban notify-unsubscribe <task-id> [--platform <P>] [--chat-id <C>]`); BUG-36.3.7.5-06 (auto-subscribe hook on `/kanban create` in `crates/ironhermes-core/src/commands/handlers.rs::cmd_kanban` — extract the originating chat from CommandContext, add the subscription, skip if `--json` flag is present); BUG-36.3.7.5-07 (receiver-end tests covering each of: schema migration, subscription CRUD, polling loop delivers exactly once per terminal event, auto-subscribe + auto-remove lifecycle, CLI surface end-to-end via the dispatch handler).
**Depends on:** Phase 36.3.7.4 (CLOSED — `reclaimed` event row confirmed emitted via `cas::release_claim` for the TTL-expiry path; receiver-end test locks the dispatcher → cas → event chain. Notifier may treat `reclaimed` as a terminal-like signal per locked decision in 36.3.7.5-CONTEXT.md)
**Plans:** 4 plans (overview at 36.3.7.5-PLAN.md + 4 sub-plans, develop-direct strategy, 2 waves)

Plans:

- [x] 36.3.7.5-01-PLAN.md — Wave 1. kanban_subscriptions schema + 5 store CRUD APIs + Subscription type + 8 receiver tests (BUG-01, -02, -07a). PASS 2026-05-30. 4 commits: `fcdab1a8` schema+type, `dd59715f` 5 CRUD methods, `8519279d` 8 receiver tests, `072c3afd` SUMMARY. All 8 Task-5 gates green. ironhermes-kanban suite 115/0. Bilateral-tracing satisfied per LEARNINGS 2026-05-29. See SUMMARY.md.
- [x] 36.3.7.5-02-PLAN.md — Wave 1. NEW notifier.rs with run_notifier_loop + run_notifier_tick + send_fn injection + notifier_poll_seconds config + 5 polling-loop receiver tests (BUG-03, -07b). PASS 2026-05-30. 4 commits: `c22febbf` notifier_poll_seconds config field (default 3), `e635e198` NEW notifier.rs (~250 LOC: NotifierContext + run_notifier_loop + run_notifier_tick + SendFn trait-object alias + format_terminal_message) + 2 store.rs helpers (list_terminal_events_after + max_event_id) + lib.rs re-exports, `f7c40e8e` 5 receiver tests in NEW tests/notifier_logic.rs (polling_loop_delivers_once_and_removes_subscription, reclaimed_event_is_ignored, watermark_advances_past_processed_event, send_fn_failure_still_removes_subscription, no_subscriptions_means_no_send), `84a94a5a` SUMMARY with bilateral-tracing table + crate-isolation audit. All 12 Task-5 gates green. ironhermes-kanban suite 120/0 (+5 from notifier_logic; exactly the planned delta from 115). Workspace suite 3552/0 — zero regressions. **Crate-isolation fence HELD** — zero ironhermes-gateway dep declarations in ironhermes-kanban/Cargo.toml; SendFn trait-object closure IS the kanban→gateway boundary (gateway injects at spawn time in Plan 03). Locked CONTEXT decisions materially observable: D-send-closure-injection, D-watermark-in-memory (AtomicI64 fetch_max), D-log-and-drop-on-fail, D-auto-remove-after-attempt. Bilateral-tracing satisfied per LEARNINGS 2026-05-29. Wave 1 of Phase 36.3.7.5 now complete; Wave 2 unblocked. See SUMMARY.md.
- [x] 36.3.7.5-03-PLAN.md — Wave 2. Gateway runner spawn + notifier_gating helper + send-closure builder + 3 gating receiver tests (BUG-04, -07c). PASS 2026-05-30. 4 commits: `16a208ed` store-arc lift refactor in runner.rs + NEW notifier spawn block mirroring dispatcher spawn at line 1278 + NEW notifier_gating.rs (pub fn compute_notifier_gate + pub enum NotifierGate) declared via pub mod notifier_gating in lib.rs + 3 private helpers (collect_enabled_platform_names + build_adapter_snapshot + build_notifier_send_fn closure with case-insensitive platform routing), `e7865757` 3 receiver-end tests in NEW tests/notifier_spawn_gating.rs (gate_returns_disabled_no_sources_when_none locking default-off for BOTH None and Some(empty), gate_returns_disabled_no_overlap_when_no_intersection with case-insensitive non-match sub-case, gate_returns_enabled_with_overlap_when_intersection_exists with 4 sub-cases incl. caller-casing preservation + multi-source filter + insertion order), `e3904f1a` SUMMARY with bilateral-tracing table + 3-level store-arc lift audit (kanban suite still 120/120 baseline + gateway INV suite still 9/9 + structural code-level audit confirms dispatcher byte-identical) + crate-isolation fence verification + 9 gate outcomes. All 9 Task-4 gates green. ironhermes-gateway suite at 165/0 (+3 from notifier_spawn_gating; exactly the planned delta from 162). ironhermes-kanban suite still 120/0 (store-arc lift confirmed semantics-preserving for dispatcher). Workspace suite 679/0 — zero NEW regressions. **Crate-isolation fence HELD** — zero ironhermes-gateway dep declarations in ironhermes-kanban/Cargo.toml; the new build_notifier_send_fn closure in gateway captures gateway-side Arcs and produces an ironhermes_kanban::SendFn (gateway→kanban flow only). **D-gateway-gating default-off preserved** — Test 1 asserts None AND Some([]) both collapse to DisabledNoSources; the runner's match arm does NOT call join_set.spawn on that branch. **One Rule-1 fix-up applied** — INV-36.3.7-08-05 (tests/kanban_dispatcher_spawned.rs:159 asserting "kanban dispatcher will NOT start" substring) initially failed because the warn message rewording broke substring contiguity; fixed by message reword preserving greppable substring AND documenting notifier-also-skipped. Discord/Slack delivery wiring deferred as permanent fence (those adapters live inside their own spawned tasks; not retained as runner-scope Arcs; subscriptions naming those platforms log+drop per locked policy). Bilateral-tracing satisfied per LEARNINGS 2026-05-29. Wave 2 half-complete; Plan 04 unblocked. See SUMMARY.md.
- [x] 36.3.7.5-04-PLAN.md — Wave 2. 3 notify-* CLI verbs + KanbanStoreWriter trait/impl + CommandContext chat-origin extension + cmd_kanban Create arm + auto-subscribe hook + gateway handler attach + 9 dispatch + lifecycle e2e tests + docs/kanban/reference.md v1-NOTE reconciliation (BUG-05, -06, -07d). PASS 2026-05-30. 7 commits: `bde6f4e2` CommandContext extension (KanbanStoreWriter trait + SubscriptionView boundary type + 3 new optional fields + 2 new builders), `845d8414` cmd_kanban Some("create") arm with auto-subscribe hook + "create" REMOVED from DEFERRED_KANBAN_SUBVERBS + platform_to_str via Platform::Display, `23842235` 3 KanbanCommands variants (NotifySubscribe/NotifyList/NotifyUnsubscribe) + 3 cmd_notify_* handlers + KanbanStoreWriterImpl (initially in cli, later relocated) + list_all_subscriptions store API, `3ba0f3d0` gateway handler.rs attaches both new builders to CommandContext + KanbanStoreWriterImpl MOVED to ironhermes-kanban (gateway needs to construct it; ironhermes-cli depends on ironhermes-gateway so reverse dep would be circular) with re-export from cli for ergonomic discoverability, `707cf01a` 9 receiver-end tests in NEW tests/handlers_kanban_notify.rs (5 BUG-05 dispatch tests + 4 BUG-06 dispatch+lifecycle tests including the keystone auto_subscribe_lifecycle_end_to_end which drives the full cross-crate pipeline: dispatch /kanban create → auto-subscribe row written → Completed event appended → run_notifier_tick reads subscription → mock send_fn invoked with (local, 42, None, message containing "lifecycle done") → subscription row removed after delivery attempt) + ironhermes-kanban added as [dev-dependencies] on ironhermes-core, `09021e84` docs/kanban/reference.md v1-NOTE reconciliation (3 "deferred to Phase 36.3.7.5" markers replaced with 3 "Shipped in Phase 36.3.7.5" annotations — Gate 11 = 0/3 hits as required), plus SUMMARY commit pending. All 11 phase-close gates green. cargo build --workspace exit 0. ironhermes-core test suite added +9 (handlers_kanban_notify) — all pass including keystone lifecycle e2e. **Crate-isolation fence HELD** — zero ironhermes-gateway dep declarations in ironhermes-kanban/Cargo.toml; KanbanStoreWriterImpl in ironhermes-kanban imports only `ironhermes_core::commands::context::{KanbanStoreWriter, SubscriptionView}` — no gateway types crossed. **D-json-skips-auto-subscribe locked decision enforced** — dispatch_kanban_create_with_json_skips_auto_subscribe asserts subscribe_calls.len() == 0 under --json. **D-auto-subscribe writes source='auto'** vs explicit notify-subscribe writes source='explicit' — CHECK constraint at storage layer (Plan 01) enforces. **One Rule-1 fix-up applied** — SubscriptionView initially had #[derive(PartialEq, Eq)] per plan verbatim; f64 created_at disqualifies Eq; dropped Eq (PartialEq remains for test assertions; semantic equality at boundary unchanged). Bilateral-tracing satisfied per LEARNINGS 2026-05-29 — BUG-05 ships producer (mod.rs + commands.rs + store_writer_impl.rs + list_all_subscriptions) AND 5-test consumer in same commit set; BUG-06 ships producer (context.rs + handlers.rs + handler.rs + store_writer_impl) AND 4-test consumer (including lifecycle e2e) in same commit set. Phase 36.3.7.5 now COMPLETE end-to-end. See SUMMARY.md.

**Phase verdict (gsd-verifier audit 2026-05-30):** **PASS** — 13/13 phase-level gates verified (12 PLAN gates + 1 implicit crate-isolation gate). Bilateral-tracing 7/7 BUGs cite both producer + consumer in same plan SUMMARYs per LEARNINGS 2026-05-29. All 9 locked CONTEXT decisions (D-table-name + schema, D-send-closure-injection, D-watermark-in-memory, D-log-and-drop-on-fail, D-json-skips-auto-subscribe, D-gateway-gating, D-single-db, D-auto-remove-after-attempt, D-37) materially observable at file:line. All 13 scope-fences upheld. Keystone test `auto_subscribe_lifecycle_end_to_end` PASS (drives full cross-crate pipeline). Phase-test re-runs by verifier: kanban `notifier_logic` 5/5 PASS, gateway `notifier_spawn_gating` 3/3 PASS, core `handlers_kanban_notify` 9/9 PASS, `cargo build --workspace` exit 0, `cargo test --workspace --no-fail-fast -- --test-threads=1` exit 0. `docs/kanban/reference.md` Gate 11 = 0 v1-NOTE hits + 3 "Shipped in Phase 36.3.7.5" annotations. Crate-isolation fence held (zero `ironhermes-gateway` dep declarations in `ironhermes-kanban/Cargo.toml`). Commit SHA range: `fcdab1a8` (Plan 01 first feat) → `f9df28f6` (Plan 04 SUMMARY) → `f67153a6` (Plan 04 state close-out). VERIFICATION report: `.planning/phases/36.3.7.5-gateway-notifier-auto-subscribe-polling-loop/36.3.7.5-VERIFICATION.md`. **Forward-compat note (NOT a failure):** Discord/Slack send_fn fan-out is documented log+drop per `D-log-and-drop-on-fail` — those adapters aren't retained as runner-scope Arcs (they live inside their own spawned tasks); future phase can hoist or add a delivery-dispatch indirection. Telegram delivery works end-to-end. Phase CLOSED PASS.

### Phase 36.3.7.6: Kanban LLM-tool surface completion — heartbeat/link/unblock (INSERTED)

**Goal:** Close the 3 LLM tools that v1's `docs/kanban/reference.md` §14 + §204-208 promised but deferred — `kanban_heartbeat`, `kanban_link`, `kanban_unblock` — extending the existing 6-tool LLM surface (`kanban_show`, `kanban_list`, `kanban_complete`, `kanban_block`, `kanban_comment`, `kanban_create`) shipped across Phase 36.3.7 + 36.3.7.0..5. After this phase, the full 9-tool LLM surface in the reference doc is honored. Each tool follows the existing pattern: JSON schema registration in `crates/ironhermes-tools/src/kanban_tools.rs` (verify exact location during phase research), handler that writes through `KanbanStore` (read-mostly for `heartbeat`; mutating for `link` + `unblock`), execution gating via `HERMES_KANBAN_TASK` (workers) OR explicit `task_id` argument (orchestrators), and full bilateral-tracing-by-construction per LEARNINGS 2026-05-30 — every tool ships producer + consumer in the same plan. The phase also reconciles the v1-NOTE markers in `docs/kanban/reference.md` §14 / §204-208 (currently point at defunct "Phase 36.3.7.1" assignment) to reflect their actual home at 36.3.7.6. CLI parity is OUT of scope unless the planner determines a tool handler requires a new CLI verb to materialize.

**Requirements**:

- BUG-36.3.7.6-01 (`kanban_heartbeat` LLM tool — no required params, optional `task_id` defaulting to `HERMES_KANBAN_TASK`; semantics: pure liveness signal. Two implementation choices the planner picks: (a) append a `Heartbeat` row to `task_events` for an audit trail, OR (b) update a `tasks.last_heartbeat_at` column without event-row write. Whichever path, the producer + receiver test pair ships together)
- BUG-36.3.7.6-02 (`kanban_link` LLM tool — required params `parent_id` + `child_id`; semantics: write a `task_links` row asserting `parent_id` → `child_id` dependency. `task_links` table already exists per `schema.rs` line ~80. Fail closed if either id doesn't exist OR if the link would form a cycle. Orchestrator-only via the same gating that `kanban_create` uses)
- BUG-36.3.7.6-03 (`kanban_unblock` LLM tool — required param `task_id`; semantics: move a `blocked` task back to `ready`. Append `Unblocked` event row (NEW `KanbanEventKind` variant — first new variant since the 36.3.7 baseline; or REUSE existing variant if one fits the shape). Mirror the existing `cmd_unblock` CLI verb's behavior if it exists; planner verifies during research)
- BUG-36.3.7.6-04 (receiver-end tests per LEARNINGS 2026-05-30 bilateral-tracing-by-construction: each of the 3 tools ships ≥2 tests — happy path + one failure path — driven through the dispatch handler the way Phase 36.3.7.5 Plan 04 drove `handlers_kanban_notify.rs`)
- BUG-36.3.7.6-05 (docs/kanban/reference.md reconciliation: update §14 v1-NOTE block to say "Shipped in Phase 36.3.7.6"; update §204, §207, §208 to drop the "deferred to Phase 36.3.7.1" annotations; verify a `grep -c 'deferred to Phase 36.3.7.1' docs/kanban/reference.md` returns 0 after this phase)

**Depends on:** Phase 36.3.7.5 (CLOSED — `KanbanStoreWriter` trait + receiver-test infrastructure in `handlers_kanban_notify.rs` are the template the new 3-tool tests follow; the new tools land alongside the existing 6 in `kanban_tools.rs` without disturbing the 36.3.7.5 surfaces). No runtime dep on the gateway notifier — these are pure-LLM-surface additions.

**Plans:** 1 plan (overview at 36.3.7.6-PLAN.md + 1 sub-plan, develop-direct strategy, 1 wave)

Plans:

- [x] 36.3.7.6-01-PLAN.md — Wave 1. 3 LLM tools (kanban_heartbeat append-event per D-heartbeat-impl / kanban_link with WITH RECURSIVE descendant-walk cycle detection inside BEGIN IMMEDIATE per D-link-cycle-detection / kanban_unblock with handler-side status-precondition gate per D-unblock-status-precondition) + 7 receiver tests in tools_smoke.rs + hermes kanban heartbeat CLI verb + 2 CLI parity tests + 4 docs/kanban/reference.md v1-NOTE reconciliation (BUG-01, -02, -03, -04, -05). PASS 2026-05-30. 6 commits: `4a11d30c` add kanban_heartbeat LLM tool + 2 tests, `3530f52a` add kanban_link with insert_link_checked + LinkCycle variant + 3 tests, `cfb7c144` add kanban_unblock with handler-side precondition + 2 tests, `78f798db` add hermes kanban heartbeat CLI verb + 2 parity tests, `11ed521e` reference.md v1-NOTE reconciliation (Gate 11 = 0 hits), `90793e58` SUMMARY with bilateral-tracing table. All 13 phase-level gates green. ironhermes-kanban tools_smoke +7 tests (14 → 21); ironhermes-cli heartbeat_cli +2 parity tests. **Crate-isolation fence HELD** — zero ironhermes-gateway dep declarations in ironhermes-kanban/Cargo.toml. **D-tool-surface-mounts-store-directly enforced** — the 3 new tools each own Arc<TokioMutex<KanbanStore>> directly, matching the existing 6-tool pattern; KanbanStoreWriter trait (36.3.7.5) NOT extended. **D-no-deferred-subverbs-change enforced** — DEFERRED_KANBAN_SUBVERBS at handlers.rs:1143-1148 untouched (still 24 entries; "link" + "unblock" still in deferred list; "heartbeat" still absent). store.insert_link + store.unblock_task signatures byte-stable. Bilateral-tracing satisfied per LEARNINGS 2026-05-30 — every BUG ships producer + consumer in same commit set. One Rule-3 fix-up applied (heartbeat_cli.rs TestSub Debug derive dropped because pre-existing KanbanCommands enum doesn't derive Debug; out-of-scope to add). See SUMMARY.md.

**Phase verdict:** Phase 36.3.7.6 CLOSED PASS. 13/13 phase-level gates verified. All 5 BUGs (heartbeat / link / unblock / receiver tests / docs reconciliation) + D-cli-heartbeat-parity ship producer + consumer in the same commit set per LEARNINGS 2026-05-30 bilateral-tracing-by-construction. All 8 locked CONTEXT decisions materially observable at file:line. All scope-fences upheld (no gateway-side slash arms; DEFERRED_KANBAN_SUBVERBS untouched; no new KanbanEventKind variants; store.insert_link / store.unblock_task signatures byte-stable; KanbanStoreWriter trait NOT extended; no tasks.last_heartbeat_at column; no external crate deps). docs/kanban/reference.md Gate 11 = 0 hits + 4 "Shipped in Phase 36.3.7.6" annotations. Crate-isolation fence held. The 9-tool LLM surface in reference.md §200-208 is now fully shipped — the v1 narrative that "IronHermes ships 6 of 9 LLM tools" is now closed; all 9 are live.

### Phase 36.3.7.7: Kanban swarm helper — multi-task fan-out for orchestrators (INSERTED 2026-05-30)

**Goal:** Ship `kanban swarm` as both a CLI verb (`hermes kanban swarm`) and an LLM tool (`kanban_swarm`) — a multi-task fan-out helper for orchestrators that creates N child tasks from a single dispatch with shared metadata (assignee defaulting per task, shared parent, shared skills set, shared workspace). Currently referenced at `docs/kanban/reference.md` §664 as deferred. Use case: an orchestrator processes a backlog of similar items (10 PRs to review, 50 docs to ingest, 20 tasks to triage) and wants atomic batch-create instead of N individual `kanban_create` calls. Includes idempotency key support (per-child suffix) and a single-transaction insert path through `KanbanStore`. Stretches the existing 9-tool surface to 10 LLM tools / matching CLI surface.

**Requirements**: TBD (run /gsd-plan-phase 36.3.7.7 to break down — research should confirm whether `KanbanStore::create_tasks_batch` exists or needs to be added as a new transactional primitive; check whether existing `create_task` can be loop-called inside a `BEGIN IMMEDIATE` transaction or whether batch-insert performance matters for v1; verify reference.md §664 wording for any locked semantics)
**Depends on:** Phase 36.3.7.6 (CLOSED — 9-tool LLM surface stable; pattern for tool registration in `tools/mod.rs` proven)
**Plans:** 1/1 plans complete

Plans:

- [x] TBD (run /gsd-plan-phase 36.3.7.7 to break down) (completed 2026-05-30)

### Phase 36.3.7.8: @mention delegation parser — inline routing from prose (INSERTED 2026-05-30)

**Goal:** Implement the `@mention` delegation parser referenced at `docs/kanban/reference.md` §741 ("P6 @mention: inline routing from prose, e.g. `@reviewer look at this`"). A worker or orchestrator can include `@<assignee>` mentions inside task bodies / comments and the dispatcher (or a dedicated handler) extracts them, creates child tasks routed to the named assignee, and threads them into the task graph. Requires: a parser for `@<word>` patterns inside Markdown bodies (with fence-escape rules — don't parse inside code blocks); an assignee resolver (map `@reviewer` to an actual assignee string); a child-task creation path that mirrors `kanban_create` but with auto-derived parent_id; and CLI/LLM hooks. Was originally reserved at "Phase 36.3.7.7" in reference.md §741; that number is now Kanban swarm helper, so this work moves down one slot. Stretches the LLM-tool surface or the dispatcher-side handler (planner decides).

**Requirements**: REQ-36.3.7.8-01..16 (see .planning/REQUIREMENTS.md §Phase 36.3.7.8). Decisions D-01..D-04 finalized 2026-05-30: D-01 LLM tool + CLI verb only (no dispatcher-tick scan); D-02 pure regex + fence-state machine (no pulldown-cmark/comrak); D-03 three-stage resolver (lowercase → validate_profile_name → existence gate) with three fallback policies (skip/pending/error); D-04 two-layer cycle defense (self-mention quick reject + ancestor-chain walk capped at MAX_MENTION_CHAIN_DEPTH=4).
**Depends on:** Phase 36.3.7.6 (CLOSED — tool registration pattern stable). No hard dep on Phase 36.3.7.7 — only soft coordination on docs/kanban/reference.md §14 tool-count narrative.
**Plans:** 5/5 plans complete

Plans:

- [x] 36.3.7.8-01-PLAN.md — mention module: parser + resolver pure functions (Wave 1)
- [x] 36.3.7.8-02-PLAN.md — store sibling primitive: create_mention_children + ancestor-walk cycle check + idempotency replay (Wave 1, parallel with 01)
- [x] 36.3.7.8-03-PLAN.md — KanbanMentionTool (11th LLM tool) + register_kanban_tools wiring (Wave 2, depends on 01+02)
- [x] 36.3.7.8-04-PLAN.md — CLI verb `hermes kanban mention` + DEFERRED_KANBAN_SUBVERBS entry + ≥4 clap parity tests (Wave 2, depends on 01+02)
- [x] 36.3.7.8-05-PLAN.md — ≥15 receiver tests in tools_smoke.rs + docs/kanban/reference.md §14/§200/§741 reconciliation + REQUIREMENTS.md anchor section (Wave 3, depends on 01+02+03)

### Phase 36.3.7.9: Multi-board CLI — `boards list/create/switch/show/rename/rm` + `--board <slug>` flag (INSERTED 2026-05-30)

**Goal:** Ship the multi-board CLI surface for IronHermes Kanban: `hermes kanban boards {list,create,switch,show,rename,rm}` (+ `--delete` on `rm`); a `--board <slug>` flag on every existing `kanban` subverb; a 4-tier `current_board` resolution chain (`--board` flag > `HERMES_KANBAN_BOARD` env > `~/.ironhermes/kanban/current` file > `default`); the multi-board on-disk layout at `~/.ironhermes/kanban/boards/<slug>/{kanban.db, workspaces/, logs/}` while the default board stays at `~/.ironhermes/kanban.db` (D-01 back-compat, no migration of existing installs); per-board `schema_version` rows with auto-migrate-with-stderr-banner (D-06); a `notifier.toml` config surface with `subscribe_boards = ["*"]` default (explicit-list codepath ships inert, unit-tested only, D-03); `boards rm` archive-by-default + hard-delete-with-open-task-refusal (D-07); LLM-tool optional `board: Option<String>` arg + always-emit `board`/`board_source` envelope fields across all 11 tools (D-08, T-5 mitigation). One dispatcher sweeps all boards per tick with per-board open-failure isolation (D-04, INV-36.3.7-08-05 extension). Cycle detection unchanged — cross-board task links are forbidden by construction so per-board WITH RECURSIVE walks remain correct (D-05).

**Requirements**: D-01, D-02, D-03, D-04, D-05, D-06, D-07, D-08 (locked CONTEXT decisions serve as requirement IDs for this phase per planning_context guidance)
**Depends on:** Phase 36.3.7.6 (CLOSED — full 11-tool LLM surface), Phase 36.3.7.5 (CLOSED — gateway notifier infrastructure), Phase 36.3.7.8 (CLOSED — mention/resolver pure-fn pattern reused)
**Plans:** 9/9 plans complete

Plans:
**Wave 1** *(no dependencies)*

- [x] 36.3.7.9-01-PLAN.md — `paths.rs` multi-board helpers + `board/` module (BoardContext, BoardSource, slug validator with T-1 path-traversal rejection, 4-tier resolve_board_context pure fn) — covers D-01, D-02

**Wave 2** *(depends on 01)*

- [x] 36.3.7.9-02-PLAN.md — `KanbanStore::open/open_labeled/open_for_board` + D-06 migration banner injection in init_schema with T-4 tx-rollback — covers D-01, D-06

**Wave 3** *(depends on 02; plans 03, 04, 08 are file-disjoint and parallel-eligible)*

- [x] 36.3.7.9-03-PLAN.md — `boards` nested clap subcommand + `cmd_boards_list/create/switch/show/rename/rm` with T-3 atomic create+switch, T-6 symlink refusal on rm, T-7 advisory file lock around hard-delete — covers D-01, D-02, D-07
- [x] 36.3.7.9-04-PLAN.md — `--board <slug>` flag plumbing through ≥22 existing `cmd_*` fns + handle_kanban_command signature extension + main.rs clap arg — covers D-02
- [x] 36.3.7.9-08-PLAN.md — Append `"boards"` to `DEFERRED_KANBAN_SUBVERBS` (single-line edit, regression test) — covers D-04 (gateway slash routing)

**Wave 4** *(depends on 01 + 02; plans 05, 06, 07 are file-disjoint and parallel-eligible)*

- [x] 36.3.7.9-05-PLAN.md — `notifier.toml` parser + workspace `toml = "0.8"` dep + multi-board NotifierContext sweep with per-board watermarks + INV-36.3.7-08-05 corrupt-board skip — covers D-03
- [x] 36.3.7.9-06-PLAN.md — Dispatcher per-tick multi-board sweep + `build_kanban_worker_env(board_slug)` env propagation + minimal gateway runner.rs change — covers D-04
- [x] 36.3.7.9-07-PLAN.md — All 11 LLM tools gain `board: Option<String>` schema param + every success/rejection envelope carries `board` + `board_source` via shared `tools/common.rs` helpers — covers D-08, T-5

**Wave 5** *(depends on all prior plans)*

- [x] 36.3.7.9-09-PLAN.md — End-to-end integration tests (boards create/switch/list/rm + 4-tier precedence + tool envelope) + D-05 no-code-change assertion + T-2 SQL-no-slug-literals static audit + docs/kanban/reference.md §71 reconciliation — covers D-05, D-08

### Phase 36.3.7.10: Auto-decompose / triage decomposer / specifier (INSERTED 2026-05-30)

**Goal:** Implement the `hermes kanban decompose` + `hermes kanban specify` CLI verbs + `kanban.auto_decompose` config knob referenced at `docs/kanban/reference.md` §425 ("Auto-decompose / triage decomposer / specifier [...] are deferred to Phase 36.3.7.2") and §744 (same feature, alternate naming "Triage specifier"). In v1, the `triage` column is a parking lot — transitions out are operator-only via `hermes kanban assign` + manual status update; the **Orchestration: Auto/Manual** toggle does not exist. Ship: an LLM-driven decomposer that takes a one-line task (`"add Stripe payments"`) and emits a body with structured fields (acceptance criteria, work-breakdown, risk register, suggested skill set); an auto-mode that runs the decomposer on `triage→todo` transitions when `kanban.auto_decompose=true`; an `Orchestration: Auto/Manual` config toggle; CLI verb that lets operators run the decomposer manually. Likely uses the `AgentRuntime` + a dedicated `decompose` skill or a delegate_task call.

**Requirements**: REQ-36.3.7.10-01 (decomposer module), REQ-36.3.7.10-02 (CLI decompose verb), REQ-36.3.7.10-03 (CLI specify verb), REQ-36.3.7.10-04 (auto_decompose config knob), REQ-36.3.7.10-05 (dispatcher Step 0), REQ-36.3.7.10-06 (failure-mode policy + DecomposeFailed event), REQ-36.3.7.10-07 (KanbanDecomposeTool + KanbanSpecifyTool LLM tools), REQ-36.3.7.10-08 (kanban_decomposer reserved role), REQ-36.3.7.10-09 (DEFERRED_KANBAN_SUBVERBS extension), REQ-36.3.7.10-10 (docs/kanban/reference.md reconciliation). Derived from ROADMAP goal + RESEARCH §Acceptance Criteria; full list in 36.3.7.10-01-PLAN.md §Requirements (Derived).
**Depends on:** Phase 36.3.7.6 (CLOSED — kanban_create + kanban_comment tools available), Phase 36.3.7.9 (CLOSED — multi-board CLI + --board flag + tools/common.rs envelope helpers + DEFERRED_KANBAN_SUBVERBS pattern). NOT dependent on Phase 36.3.7.7 (swarm) — RESEARCH §Q8 + §Assumptions A3 show decompose-children land via a new store.apply_decompose mirroring create_swarm's atomic-tx shape but NOT calling create_swarm directly (the "root" is a pre-existing triage task, not a new card).
**Plans:** 6/6 plans complete

Plans:
**Wave 1** *(no dependencies)*

- [x] 36.3.7.10-01-PLAN.md — Foundations: KanbanConfig +6 fields + KanbanEventKind +3 variants + RESERVED_ROLE_NAMES 7→8 (`kanban_decomposer`) + DEFERRED_KANBAN_SUBVERBS 26→28 (`decompose`/`specify`) — covers REQ-04, REQ-06 (event variant), REQ-08, REQ-09

**Wave 2** *(depends on 01)*

- [x] 36.3.7.10-02-PLAN.md — Decomposer kernel: NEW decomposer.rs (DecomposeFn typedef + 4 data structs + specify_triage_task + decompose_triage_task) + 2 new store helpers (apply_specify + apply_decompose with WHERE status='triage' guard + BEGIN IMMEDIATE) + lib.rs re-exports + 6 receiver tests (DEC-01..06) — covers REQ-01, REQ-06 (policy wired)

**Wave 3** *(depends on 02; plans 03 + 04 are file-disjoint and parallel-eligible)*

- [x] 36.3.7.10-03-PLAN.md — LLM tool surface: NEW tools/decompose.rs + tools/specify.rs (KanbanDecomposeTool + KanbanSpecifyTool mirror tools/create.rs shape; v1 returns no_aux_client envelope per crate-isolation fence) + register_kanban_tools tool-count regression test 11→13 — covers REQ-07
- [x] 36.3.7.10-04-PLAN.md — Dispatcher auto-mode: DispatcherContext.decompose_fn field + run_dispatch_tick_for_board Step 0 (gated on config.auto_decompose && decompose_fn.is_some()) + decompose_triage_tasks helper (sequential, per-tick-capped) + 3 receiver tests — covers REQ-05, REQ-06 (dispatcher invocation)

**Wave 4** *(depends on 01 + 02)*

- [x] 36.3.7.10-05-PLAN.md — CLI verbs: KanbanCommands::Decompose + Specify variants + dispatch arms + cmd_decompose + cmd_specify + build_runtime_decompose_fn (three-tier model cascade: kanban.decomposer_model > auxiliary.kanban_decomposer > main provider) + ≥4 clap parity tests in NEW tests/decompose_cli.rs — covers REQ-02, REQ-03

**Wave 5** *(depends on all prior plans)*

- [x] 36.3.7.10-06-PLAN.md — Docs reconciliation: replace `deferred to Phase 36.3.7.10` at §425 + §744 with `Shipped in Phase 36.3.7.10` blocks; add `hermes kanban decompose` line to §609 CLI listing; annotate §444 config row with IronHermes v1 default override — covers REQ-10

### Phase 36.3.7.11: Dashboard plugin — SPA + REST + WebSocket live-update for hermes dashboard Kanban tab (INSERTED 2026-05-30)

**Goal:** Implement the dashboard plugin referenced at `docs/kanban/reference.md` §387 ("The dashboard plugin is deferred to Phase 36.3.7.4. [...] The `hermes dashboard` command will open the Kanban tab once 36.3.7.4 ships."). The `/kanban` slash command + CLI surface work today; the dashboard SPA / REST API / WebSocket live-update layer does not exist. Ship: a Kanban tab in `iron_hermes_ui` (HermesApp — see project memory) backed by a REST API exposing the existing 9 kanban_* tool surface PLUS a `tasks/list` + `tasks/get` + `events/poll` endpoint set; a WebSocket subscription that pushes `task_events` rows live as the dispatcher emits them (the 36.3.7.5 notifier is already polling, but the dashboard wants pub/sub semantics not poll); a Kanban-board view (4-column ready/running/blocked/done with drag-to-status); a per-task detail view with the same `worker_context` shape that `kanban_show` returns. Large scope — likely a multi-plan phase (REST layer + WebSocket + UI components + integration).

**Requirements**: D-01 through D-23 from .planning/phases/36.3.7.11-dashboard-plugin-spa-rest-websocket-live-update/36.3.7.11-CONTEXT.md (no canonical REQ-IDs — phase scope is fully captured by D-NN locked decisions)
**Depends on:** Phase 36.3.7.6 (CLOSED — full 9-tool LLM surface available). Phase 36.3.7.10 referenced for Decompose/Specify wiring (Q9 branch decision in Plan 02).
**Plans:** 5/5 plans complete

Plans:

- [x] 36.3.7.11-01-PLAN.md — Walking skeleton: read-side #[server] fns (fetch_board/fetch_task/fetch_task_events/fetch_task_runs/fetch_comments) + /api/ws/kanban WS tail consumer + ScreenKanban with 6 columns + Screen::Kanban variant + list_all_events_after + DashboardConfig + Cargo.toml deps. Foundational. Covers D-02, D-04, D-05, D-08, D-09, D-15, D-16, D-17, D-18, D-19, D-22, D-23.
- [x] 36.3.7.11-02-PLAN.md — Drag-and-drop + four write #[server] fns (patch_task_status / post_comment / create_task / run_decompose_or_specify) + kanban::transitions shared validator + keyboard DnD alternative. Depends on 01. Covers D-06, D-07, D-10, D-11, D-13, D-14, D-19.
- [x] 36.3.7.11-03-PLAN.md — Detail drawer (7 sections per D-20) + four modals (Complete/Block/Archive/Create) + per-task event counter (D-21) + comment compose + TRIAGE Decompose/Specify wiring. Depends on 01. Covers D-12, D-13, D-20, D-21.
- [x] 36.3.7.11-04-PLAN.md — WheelWedge::Kanban (atomic 6-method modulo-11 update — Risk 1) + wheel.rs geometry + Agents page KANBAN BOARD → button. No deps; can ship first. Covers D-02, D-03.
- [x] 36.3.7.11-05-PLAN.md — UAT: full workspace gate (build/test/clippy) + kanban_ws_lifecycle.rs + kanban_full_suite.rs aggregating must_haves invariants + blocking manual UAT checkpoint with 14-row checklist. Depends on 01-04.

### Phase 36.3.7.12: Goal mode - kanban worker loop (Ralph loop) (INSERTED)

**Goal:** Extend kanban auto-decompose infrastructure with a per-card goal_mode opt-in that, when set, makes the spawned worker enter an in-session Ralph-style loop. Each turn the worker output is evaluated by an auxiliary judge LLM against the card's title + body (literal acceptance criteria). Loop terminates on judge-agrees, worker self-terminates (kanban_complete/kanban_block), or per-card turn budget exhaustion → synthetic kanban_block(reason="goal_max_turns exhausted; needs human review"). NO new dispatcher pool, NO new event variants (Edited+subkind frozen-surface pattern), NO DEFCON gating (deferred).
**Requirements**: D-01..D-08 (locked in CONTEXT.md; no pre-assigned REQ-IDs — anchors are D-XX per phase 36.3.7.7 / 36.3.7.8 precedent)
**Depends on:** Phase 36.3.7.11
**Plans:** 5/5 plans complete

Plans:

- [x] 36.3.7.12-01-PLAN.md — Schema + types foundation (SCHEMA_VERSION 1→2, goal_mode/goal_max_turns/goal_turns_used columns, Task + CreateTaskOptions fields, KanbanConfig.judge_model, JudgeFn typedef in new judge.rs, kanban_judge reserved-role registration). Wave 1.
- [x] 36.3.7.12-02-PLAN.md — Producer surface (kanban_create JSON schema fields, worker_spawn HERMES_KANBAN_GOAL_* env injection, CAS-gated bump_goal_turn_counter, GOAL_SUBKIND_* CONSTs, frozen-surface event-variant guard). Wave 2, parallel with 03.
- [x] 36.3.7.12-03-PLAN.md — CLI surface (--goal/--goal-max-turns clap args, cmd_create signature + show formatter, build_runtime_judge_fn three-tier model cascade). Wave 2, parallel with 02.
- [x] 36.3.7.12-04-PLAN.md — Goal loop wrapper (new file goal_loop.rs with run_goal_loop_if_enabled, BudgetSentinel RAII drop guard, synthetic kanban_block helper, main.rs worker-mode dispatch wiring, 5 behavioral consumer tests). Wave 3.
- [x] 36.3.7.12-05-PLAN.md — Integration tests + docs + UAT (dispatcher passthrough test, reclaim-resets-counter wiring, kanban-worker skill amendment, docs/kanban/reference.md Goal mode section, workspace gate, 4-row manual UAT checkpoint). Wave 4. Has blocking human-verify checkpoint.

### Phase 36.3.7.13: Kanban worker cross-profile ergonomics + goal-mode UAT polish (INSERTED)

**Goal:** Close the operational gaps surfaced during Phase 36.3.7.12 goal-mode UAT. (1) Add `KanbanStore::open_from_env()` consuming `HERMES_KANBAN_DB` so dispatcher + worker can run under different profiles without losing the card (F-01 — operator hit silent cross-profile DB mismatch during live UAT). (2) Add `IRONHERMES_WORKER_BIN` env override on `worker_spawn.rs:254` so `cargo run` from a worktree can pin worker subprocesses to the same code base (F-02 — operator's `~/.local/bin/ironhermes` symlink ran May-31 binary silently). (3) Default goal-mode workers to a RESTRICTED toolset (no terminal, no delegate_task, no web) and clamp inner agent-loop `max_iterations` ≤10 when `HERMES_KANBAN_GOAL_MODE=1` (F-03 — eliminated 10-15 min/turn thrashing). (4) Ship six documentation surfaces (F-04, F-05): `docs/configuration/profiles.md` (NEW), `docs/kanban/cli-reference.md` (NEW), `docs/kanban/profile-discipline.md` (NEW), `docs/kanban/goal-mode-uat-prompts.md` (NEW), `skills/kanban-worker/SKILL.md` (UPDATE), `docs/kanban/reference.md` (UPDATE). Schema bump v2→v3 for new `goal_toolset TEXT NULL` column. No new dispatcher pool, no new gateway-side edits, crate-isolation fence holds.
**Requirements**: D-A1, D-A2, D-B1, D-B2, D-B3, D-C1, D-D1, D-E1, D-E2, D-F1, D-G1, D-H1, F-02, Schema-v3 (ratified in 36.3.7.13-CONTEXT.md).
**Depends on:** Phase 36.3.7.12
**Plans:** 3/4 plans executed

Plans:

- [x] 36.3.7.13-01-PLAN.md — F-01 cross-profile DB resolution: KanbanStore::open_from_env() + open_from_env_or_board(slug) + swap 25 dispatcher-bridged sites (CLI/gateway/dashboard/11 LLM tools); 5 bilateral tests (D-A1/D-A2/D-H1). Wave 1, parallel with 02.
- [x] 36.3.7.13-02-PLAN.md — F-02 IRONHERMES_WORKER_BIN override: resolve_worker_bin() helper + Command::new swap + SAFE_SYSTEM_VARS 8th entry; 4 bilateral tests. Wave 1, parallel with 01 (file-disjoint).
- [x] 36.3.7.13-03-PLAN.md — F-03 restricted toolset + schema v3 + inner-loop clamp: goal_toolset TEXT NULL column + v2→v3 migration + ToolRegistry::retain_by_name + filter_for_goal_mode_if_applicable + --goal-toolset clap + KanbanCreateTool schema + KanbanConfig.goal_inner_max_iterations (D-B1/B2/B3/E1/E2/F1/G1/Schema-v3); 13 bilateral tests (4 migration + 3 dispatch + 6 filter). Wave 2, depends on Plan 01.
- [ ] 36.3.7.13-04-PLAN.md — F-04 documentation: 6 doc surfaces (DOC-A profiles.md NEW + DOC-B cli-reference.md NEW + DOC-C profile-discipline.md NEW + DOC-D goal-mode-uat-prompts.md NEW + DOC-E SKILL.md UPDATE + DOC-F reference.md UPDATE) + §5 LIVE UAT human-verify checkpoint + §6 dual-rail evidence. Wave 3, depends on Plan 03.

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
