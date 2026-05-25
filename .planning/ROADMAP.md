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

### Phase 36.6.1: BUG FIX: AI response visibility — investigate why streamed/final AI responses don't render visibly in ratatui TUI; verify whether feedback_scroll_width_inner formulas (area.width-2 inner, prefix+body+width-1/width, viewport_content_length) ever landed; ship a regression test (INSERTED)

**Goal:** [Urgent work - to be planned]
**Requirements**: TBD
**Depends on:** Phase 36.6
**Plans:** 0 plans

Plans:
- [ ] TBD (run /gsd-plan-phase 36.6.1 to break down)

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

**Goal:** [Urgent work - to be planned]
**Requirements**: TBD
**Depends on:** Phase 36.3
**Plans:** 0 plans

Plans:
- [ ] TBD (run /gsd-plan-phase 36.3.7 to break down)

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

### Phase 36.2: Agent loop & core parity — prompt caching, per-provider rate-limit tracking, usage/cost accounting, error classification (INSERTED)

**Goal:** Close the four AGENT-LOOP & CORE parity gaps from iron-hermes-planning §2.1 against hermes-agent v0.14.0 as four interlocking sub-systems sharing a typed classification source. (A) Anthropic-native prompt caching via `cache_control` breakpoints with `system_and_3` strategy (PRMT-08/PRMT-09); configurable TTL 5m/1h default 1h; cache_read/cache_creation Usage fields populated end-to-end; cache-break warnings on the existing PRMT-10/11 channel for the 3 known triggers (model swap, memory edit, context-file edit); strict cached/ephemeral layer assertion at assembly time. (B) Per-provider in-memory `RateLimitTracker` in `ironhermes-agent` keyed on (provider, api_key_hash, model); reads anthropic-ratelimit-* and x-ratelimit-* headers (Anthropic uses RFC 3339 reset, OpenRouter/Nous use float-seconds); reactive 429 fallback when headers absent; emits structured `RateLimitEvent` on a Tokio broadcast channel; api_key_hash is SHA-256-truncated-16-bytes via sha2 0.10.9 (BLAKE3 not in Cargo.lock); cleartext keys never serialize anywhere. (C) Usage/cost ledger via static `pricing.toml` + disk cache at `$HERMES_HOME/pricing-cache.json` + `hermes pricing refresh` CLI; schema migration v9 adds 3 cache+cost columns to `sessions` and creates `usage_events` table with per-turn rows + 2 indexes; every LLM call (success or failure) writes one row atomically with the sessions UPDATE inside a single rusqlite transaction; failed calls write `error_kind = Some(ProviderError::variant_name())` with cost=0; subagent rows tagged with subagent session_id; surfaces via `/usage` slash command + TUI status-bar `$X.YYY │ N.NK tok` pill + per-turn `tracing::info!`. (D) Typed `ProviderError` enum (11 variants: RateLimited, Auth, Billing, ContextLength, Server, Transport, SchemaInvalid, ToolError, ModelNotFound, PayloadTooLarge, Unknown) — additive wrap of the existing `classify_llm_error` (now a `From<ProviderError> for (bool, bool)` facade); ports 30 hermes-agent `error_classifier.py` test cases verbatim into Rust as the parity bar; the typed enum is the canonical classification source consumed by sub-system B (`RateLimited{retry_after}` destructure) and sub-system C (`variant_name()` stored in `usage_events.error_kind`). The existing 12+ Rust classifier tests at agent_loop.rs:2585-2789 pass byte-for-byte verbatim. Anthropic billable input cost formula sums (input_tokens + cache_read + cache_creation) per appropriate per-million rate (Anthropic `input_tokens` field is post-last-breakpoint only). All cost arithmetic is i64 micro-USD integers; f64 conversion is gated to (a) the network refresh boundary and (b) the display layer.
**Requirements**: PRMT-08, PRMT-09 (closed by Area A). Coordinates with PRMT-10, PRMT-11, PROV-07, PROV-09, PROV-10 (no closure).
**Depends on:** Phase 36
**Plans:** 11/11 plans complete

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


### Phase 36.1: Running-agent guard parity — web UI + TUI (INSERTED)

**Goal:** Extend the Phase 36 per-session running-agent guard (`RunningAgentGuard` RAII, `is_bypass`, D-02 rejection message) to the web UI and TUI surfaces, closing the parity gap identified in `36-BACKLOG.md` items 1 and 3. Two tracks: (A) **Web UI** — add per-WebSocket-session `Arc<AtomicBool>` running flag to `iron_hermes_ui` session state, wire `RunningAgentGuard` at `run_web_turn` entry, add slash-command interception before `run_turn` so `/model` and other state-mutating commands are rejected with the D-02 message while an agent turn is active, and `/stop`/`/new` bypass; (B) **TUI** (`tui_rata`) — replace the `pending_rx.is_some()` ad-hoc check at `tui_rata/commands.rs:537` with the same `Arc<AtomicBool>` + RAII pattern, wire guard at TUI turn entry, add command interception with bypass list parity. Extract shared `RunningAgentState` + `RunningAgentGuard` to `ironhermes-core` (or a shared module) so all three surfaces (gateway, web, TUI) use one canonical implementation — mirrors the `MemoryManagerHandle`/`McpReloader` trait patterns from Phases 20/21.2.
**Requirements**: GW-05-WEB (web UI guard parity), GW-05-TUI (TUI guard parity) (phase-local; defined during /gsd:discuss-phase 36.1)
**Depends on:** Phase 36
**Plans:** 4/4 plans complete

Plans:

- [x] 36.1-01-PLAN.md — Extract RunningAgentGuard + is_bypass + AGENT_RUNNING_REJECT_MSG to ironhermes-core::commands::running_agent; update gateway imports + preserve re-export so Phase 36 suite passes unchanged
- [x] 36.1-02-PLAN.md — Web UI: AppState.running_agents per-session map, ws.rs slash interception + plain-text guard, run_web_turn RAII guard, 6 GW-05-WEB integration tests
- [x] 36.1-03-PLAN.md — TUI: App.agent_running persistent field, commands.rs snapshot replacement + bypass check (Pitfall 4), event_loop.rs spawn_turn guard INSIDE tokio::spawn async block (Pitfall 1), 5 GW-05-TUI integration tests
- [x] 36.1-04-PLAN.md — Cross-surface verification: full workspace test + clippy; fill 36.1-VALIDATION.md per-task map; sign off nyquist_compliant true
