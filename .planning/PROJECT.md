# IronHermes

## What This Is

A Rust rewrite of [hermes-agent](https://github.com/NousResearch/hermes-agent), the self-improving AI agent by Nous Research. IronHermes is a single-binary, high-performance conversational AI agent that runs as a CLI tool and Telegram bot — with automation capabilities including scheduled tasks, event hooks, a skills system, Python code execution, subagent delegation, and batch processing.

## Core Value

A working conversational AI agent with personality (context files) that operates reliably over Telegram — the core loop of receive message, think with tools, respond must work flawlessly.

## Requirements

### Validated

- ✓ Context file loading (SOUL.md, AGENTS.md, project context) with priority chain assembly — v1.0
- ✓ Telegram gateway with streaming, concurrent users, graceful shutdown, error recovery — v1.0
- ✓ Self-improvement: agent reads/edits own context files with injection scanning — v1.0
- ✓ Memory subsystem with bounded facts in MEMORY.md — v1.0
- ✓ Web scraping via Firecrawl + local fallback with SSRF protection — v1.0
- ✓ Security: SSRF validation, threat scanning, gateway rate limiting — v1.0
- ✓ Scheduled tasks with natural language parsing, skill attachment, multi-platform delivery — v1.1
- ✓ Event hooks with lifecycle logging, guardrail tool interception, webhook forwarding — v1.1
- ✓ Skills system with progressive disclosure, agentskills.io compatibility, allowed_tools enforcement — v1.1
- ✓ Code execution sandbox with Python RPC tool bridge, env stripping, resource limits — v1.1
- ✓ Subagent delegation with isolated context, concurrency control, batch mode, cancellation — v1.1
- ✓ Batch processing with parallel ShareGPT output, checkpointing, quality filtering — v1.1

### Active

<!-- Current scope for v2.0: Intelligence & Identity -->

- [ ] Persistent memory (MEMORY.md/USER.md bounded stores, memory tool with add/replace/remove, substring matching, capacity management, security scanning)
- [x] Memory provider trait with pluggable backend abstraction, lifecycle hooks, and config-driven factory — Validated in Phase 11: Memory Provider Trait
- [ ] Memory provider backends: SQLite, Grafeo (graph DB), and DuckDB; single-provider selection
- [ ] Session storage (SQLite state.db, WAL mode, sessions/messages tables, FTS5 full-text search, schema migrations, write contention handling, session lineage, session_search tool)
- [x] Context compression (dual system: gateway hygiene at 85%, agent ContextEngine at 50%; pluggable ContextEngine trait, structured summaries, iterative re-compression) — Validated in Phase 18: Context Compression (live UAT Test 2 re-run pending)
- [ ] Prompt caching (Anthropic cache_control breakpoints, system_and_3 strategy, cached/ephemeral separation)
- [x] Prompt assembly (10-layer system prompt builder matching hermes-agent: SOUL.md identity, tool-aware guidance, memory snapshots, skills index, context files, timestamps, platform hints) — Validated in Phase 15: 10-Layer Prompt Assembly
- [x] Context file loading (.hermes.md > HERMES.md > AGENTS.md > CLAUDE.md > .cursorrules priority chain, .cursor/rules/*.mdc fallback, progressive subdirectory discovery with 8K cap, security scanning, truncation) — Validated in Phase 15: 10-Layer Prompt Assembly
- [x] SOUL.md personality system (durable identity from HERMES_HOME, default fallback, /personality session overlays, 14 built-in presets, custom preset loading) — Validated in Phase 15: 10-Layer Prompt Assembly
- [ ] Skill framework (SKILL.md format, category-based discovery, progressive disclosure, conditional activation, env var/config requirements, credential file mounting, security scanning, Skills Hub)
- [x] Slash command integration (platform-agnostic CommandRouter with 49 commands, three-stage resolution, unified CLI+gateway dispatch) — Validated in Phase 21.1: Slash Commands
- [ ] Tool registry improvements (toolset management, check functions, setup wizard integration)
- [x] CLI feature parity (execute_code, hooks, guardrails available in CLI mode) — Validated in Phase 22: CLI Feature Parity
- [ ] Configuration/setup wizard improvements

### Out of Scope

- Discord/Slack adapters — foundation first, additional platforms after Telegram is solid
- Web UI — CLI and Telegram cover the primary use cases
- Multi-user auth system — single-operator deployment for now
- Plugin/extension system — tools are compiled-in; dynamic loading is premature complexity

## Context

- Ported from a ~277K line Python project; Rust version is ~360K lines across 7 workspace crates
- v1.0 shipped 2026-04-08: core agent loop, Telegram gateway, self-improvement, web scraping
- v1.1 shipped 2026-04-11: scheduled tasks, event hooks, skills, code execution, subagents, batch processing
- v2.0 Phase 21 complete 2026-04-17: CLI TUI polish (status bar, knight-rider scanner, graceful double ctrl-c)
- v2.0 Phase 22 complete 2026-04-17: CLI feature parity (cron, skills, execute_code, guardrails, HookRegistry in both CLI paths)
- v2.0 Phase 22.1 complete 2026-04-17: TUI extension hooks (TuiExtension trait, widget slot compositing, keybinding registry, command dispatch chain, render/REPL integration)
- v2.0 Phase 21.1 complete 2026-04-18: Slash commands (CommandRouter with 49 commands, three-stage resolve, unified CLI+gateway dispatch replacing hardcoded handlers)
- v2.0 Phase 21.3 complete 2026-04-20: Model metadata (ModelRegistry with 37-model static table + disk cache, tiktoken-rs token estimation, models.dev/OpenRouter API fetch, CLI subcommands + slash commands, D-06 context_length precedence chain)
- v2.0 Phase 21.4 complete 2026-04-20: Persistent memory gap closure (memory_manager wired into AgentLoop/context engine across CLI+gateway, memory_enabled/user_profile_enabled config toggles, `hermes memory status/off` subcommands, on_session_end in clean exit paths, MEM-06 verified)
- v2.0 Phase 21.5 complete 2026-04-21: Memory provider plugin (factory config loading, SQLite FTS5 memory_recall, Grafeo entity extraction, DuckDB ILIKE bridge, agent loop wiring for memory provider tools)
- v2.0 Phase 21.6 complete 2026-04-22: Deployment setup files (.env.example, cli-config.yaml.example, Dockerfile with multi-stage Rust build, docker/entrypoint.sh, install.sh curl-pipe installer, setup-ironhermes.sh dev setup, ensure_home_dirs() first-run scaffolding)
- v2.0 Phase 22.4.2.2 complete 2026-04-27: Cron create defaults to TG origin when gateway active — both `hermes cron create` (CLI) and the LLM `cronjob` tool auto-route to `deliver=origin` for the configured single-chat whitelist; multi-chat falls back to `local` with operator hint (stderr from CLI, `tracing::warn` from LLM tool); explicit `--deliver` flag/JSON arg preserved as full bypass; OriginDecision enum lives in `ironhermes-core` with plain-String fields to avoid a circular crate dep; INV ledger advanced 62 → 64
- v2.0 Phase 22.4.2.3 complete 2026-04-27: Fix pre-existing INV-22.3-02 banner-bleed regression test on `develop` — relaxed `assert_eq!(count, 1)` to `assert!(count >= 1)` and tightened ordering via `match_indices` so every `print_banner();` call site is asserted strictly before `TuiHandle::new_with_extensions`; accepts the three legitimate Plan 22.4-11 ratatui-arm + run_chat sites without losing regression intent; structural-test-only edit, `main.rs` byte-identical; 6/6 invariants_22_3 tests green
- v2.1 Phase 26.3 complete 2026-05-05: Persistent chromiumoxide user_data_dir (BrowserConfig.user_data_dir field, spawn() wired to $HERMES_HOME/browser-profile default, ensure_home_dirs() scaffold — closes cookie/localStorage loss bug on every browser_close)
- 400+ workspace tests passing
- The "self-improving" aspect is the project's differentiator — the agent edits its own SOUL.md/AGENTS.md to refine its personality and capabilities over time
- Tech stack: Rust 2024 edition, tokio async, SQLite (rusqlite), OpenAI-compatible LLM API

## Constraints

- **Language**: Rust 2024 edition — committed, no mixed Python/Rust
- **Async runtime**: tokio — already threaded through all crates
- **Database**: SQLite via rusqlite — embedded, no external DB dependency
- **LLM API**: OpenAI-compatible endpoints (OpenRouter, Nous, Anthropic) — no vendor lock-in
- **Deployment**: Single binary — `cargo build --release` produces one artifact
- **Config**: YAML + .env at `~/.ironhermes/` — established pattern, don't change

## Key Decisions

| Decision | Rationale | Outcome |
|----------|-----------|---------|
| Cargo workspace with 7 crates | Mirrors Python module structure, enables independent compilation and testing | ✓ Good |
| anyhow for all error handling | Speed of development over typed errors; HermesError enum exists but unused | ✓ Good |
| In-memory SessionStore for gateway | Simple start; can migrate to SQLite StateStore later | ✓ Good |
| Telegram first, other platforms later | Python hermes-agent's primary deployment is Telegram | ✓ Good |
| Context files over database for personality | Matches Python approach; files are git-trackable and agent-editable | ✓ Good |
| Phase ordering SCHED → HOOK → SKILL → EXEC → AGENT → BATCH | Hooks early for observability of later features | ✓ Good |
| New crates: ironhermes-hooks, ironhermes-exec | Clean separation of concerns for hooks and code execution | ✓ Good |
| Skills in ironhermes-core, SkillsTool in ironhermes-tools | No new crate deps needed | ✓ Good |
| Pattern-based env exclusion for exec sandbox | Forward compatible with new env vars | ✓ Good |
| delegate_task excluded from child toolsets | Structural recursion prevention | ✓ Good |
| Gateway-only for execute_code/hooks/guardrails | CLI is minimal interactive mode; gateway is full-featured | ⚠️ Revisit — v2 brings CLI parity |
| Cross-crate transport types use plain Strings (no embedded downstream types) | `OriginDecision` in `ironhermes-core` carries `String` fields, not `ironhermes_cron::JobOrigin` — embedding would create a circular crate dep. Consumers (CLI + LLM tool) construct `JobOrigin` at the call site where both crates are in scope. Pattern applies to any future enum that returns "what platform/route to use" data from `ironhermes-core` to a downstream crate. | ✓ Good (Phase 22.4.2.2) |

## Current Milestone: v2.1 Carry-Overs + Learning Loop

**Goal:** Close all v2.0 deferred requirements (29 carry-overs across 7 categories) **AND** land the Learning Loop foundation (5 new reqs in 2 phases). Together, these make IronHermes self-improving rather than just feature-complete — the Learning Loop is the canonical hermes-agent differentiator (per the architecture article that informed v2.1 planning).

**Target features (34 reqs across 8 categories, 11 phases):**

*Carry-overs (29 reqs / 9 phases):*
- ACP adapter for IDE integration: JSON-RPC stdio server + SessionManager + event/permission/tool bridges + cwd-bound sessions (CLI-03..08, 6 reqs)
- Anthropic prompt caching: cache_control breakpoints + system_and_3 strategy + cached/ephemeral separation (PRMT-08, PRMT-09, 2 reqs)
- Toolset management: registry improvements, check_fn requirements, setup wizard hooks, runtime enable/disable (TOOL-01..05, 5 reqs)
- Provider polish: API key per-base-URL scoping, auxiliary model routing, named custom providers (PROV-04, PROV-06, PROV-08, 3 reqs)
- Skills trust tiers: replace hardcoded `Community` with builtin/official/trusted/community/self-created discrimination (SKILL-09, 1 req)
- Gateway formal verification: back-fill formal verification of existing `ironhermes-gateway` crate (GW-01..04, GW-06, GW-07, GW-09, GW-10, 8 reqs)
- Configuration / setup wizard: `hermes setup`, `config set/get/show`, `config migrate`, profile isolation (CFG-01..04, 4 reqs)

*Learning Loop (5 NEW reqs / 2 phases):*
- Periodic nudge mechanism + memory persistence judgment: at configurable intervals, agent decides per-item which memory layer (prompt vs session-search) information belongs in (LEARN-01, LEARN-02, 2 reqs)
- Autonomous skill creation + skill_manage tool with patch-preferred semantics: agent detects task patterns worth documenting, writes/refines SKILL.md autonomously via 6-action skill_manage tool (LEARN-03, LEARN-04, LEARN-05, 3 reqs)

### Architectural Principles (carried through every v2.1 phase)

These principles, sourced from the canonical hermes-agent architecture, must be honored across all v2.1 phase implementations. They are not isolated to one phase — they constrain every phase's design.

1. **The Learning Loop is the unifying philosophy** — Skills + Memory + Session Search are outputs of one continuous self-improvement process, not separate features. Phases 32-33 land the foundation; every other phase must consider how it participates.
2. **Cache-awareness is load-bearing** — Three operations break the prompt cache: (a) switching models mid-session, (b) changing memory files mid-session, (c) changing context files mid-session. Phase 27 enforces; Phases 23/25/26 must surface warnings when their config could trigger it.
3. **3,575 char total memory limit** — already aligned (MEM-01 2,200 + MEM-02 1,375). Phase 32's nudge mechanism must respect this when persisting.
4. **Patch-over-rewrite for skill self-improvement** — Phase 33's `skill_manage` defaults to patch action for token efficiency + correctness preservation (mirrors the existing memory tool's substring-matching pattern).
5. **Progressive disclosure for token economy** — names + summaries always; full content on demand. Applies to skills (Phase 28) and is a design constraint Phase 33 must preserve.
6. **Sessions tied to ID, not platform** — cross-platform continuity. Phase 29 verifies; Phases 30/31 implement for ACP.
7. **Gateway as same-loop participant**, not bolt-on delivery — incoming messages can trigger skill creation (Phase 33), automation outputs route back through gateway. Phase 29 verifies.

**v2.0 outcome (closed 2026-04-27, status: tech_debt):**
- 77/93 active requirements satisfied (~83%); 5/5 cross-phase integration; 4/4 user flows
- Shipped: persistent memory subsystem, session storage + FTS5, context compression, 10-layer prompt assembly, context file loading, SOUL.md personality, skill framework + Hub + remote install, slash commands (49), CLI tool parity, ratatui-backed TUI, cron with TG origin routing, MCP client + slash integration, model registry + token estimation, multi-agent + autonomous + sandbox, deployment setup files
- Audit: `.planning/v2.0-MILESTONE-AUDIT.md`

**v2.2 reservation (Production Polish, ~3 months):**
After v2.1, the next milestone targets daily-driver tool maturity: credential pools + multi-provider OAuth (NEW), self-update + uninstall (deferred from v2.0 informal scope), smart model routing (NEW), plus any v2.1 carry-overs that didn't ship.

**Future Requirements parking lot:**
14 GAP-NEW items identified during v2.1 planning (Voice STT/TTS, Vision, Image gen, Browser, Profiles, Plugins, Pairing, Insights, MoA, Tirith, Honcho, Shell completions, Clarify toolset, plus Smart routing pre-reservation) parked in REQUIREMENTS.md → Future Requirements. Re-evaluate at each milestone planning.

**Architectural constraint:** All implementation must align to hermes-agent's architecture (see hermes-agent Architecture docs). Port faithfully, deviate only with documented rationale.

## Evolution

This document evolves at phase transitions and milestone boundaries.

**After each phase transition** (via `/gsd-transition`):
1. Requirements invalidated? → Move to Out of Scope with reason
2. Requirements validated? → Move to Validated with phase reference
3. New requirements emerged? → Add to Active
4. Decisions to log? → Add to Key Decisions
5. "What This Is" still accurate? → Update if drifted

**After each milestone** (via `/gsd-complete-milestone`):
1. Full review of all sections
2. Core Value check — still the right priority?
3. Audit Out of Scope — reasons still valid?
4. Update Context with current state

---
*Last updated: 2026-05-22 — Phase 34b (context-system parity) complete: @-reference expansion + ContextEngine lifecycle hooks + ContextCompressor reset, with context_warnings rendered out-of-band at all three surfaces (WR-01 closed), verified 16/16. 3 human-UAT items pending live confirmation. See Context above.*

*Earlier: 2026-05-12 — Phase 26.3.2 (Chrome singleton user browser-profile) complete. Closed the stale-`SingletonLock` gap from Phase 26.3: added `ironhermes_core::browser_profile::reconcile_singleton_lock(&Path) -> SingletonOutcome` — a dep-free helper (no `nix`/`libc`/`sysinfo`/`hostname` crate; `/proc/<pid>` on Linux else `kill -0`, `hostname` CLI for the host check) that `read_link`s `$HERMES_HOME/browser-profile/SingletonLock`, parses `<host>-<pid>`, and on a dead pid or wrong-host target removes `SingletonLock`+`SingletonSocket`+`SingletonCookie` (best-effort) then reuses the persistent profile; on a live pid returns `UseEphemeral` (caller skips `builder.user_data_dir(...)` → pre-26.3 ephemeral temp dir, `warn!` logged, launch still succeeds); absent/unparseable is a no-op `UseProfile` (nothing deleted). Wired into both `browser_session.rs` `spawn()` call sites (`ironhermes-tools` + the workspace-excluded `ironagent-tools-api` twin, kept byte-identical) right before the `.user_data_dir(...)` decision; non-Unix is a `#[cfg(not(unix))]` no-op returning `UseProfile`. 8 unit tests (SL-01..SL-07 + extra); 10/10 must-haves (D-01..D-10) verified; live planted-stale-lock → `browser_navigate` smoke test passed (user). Code review: 0 blockers, 3 advisory warnings (live-lock could be deleted on hostname-CLI vs `gethostname` divergence, on a restricted `PATH` making the probe spawn fail, or on macOS where `kill -0` collapses ESRCH/EPERM) — accepted; a cross-process browser mutex stays deferred.*

*Earlier: 2026-05-12 — Phase 27.1.4.1.1 (fallback on transport errors not just HTTP status) complete. Closed the PROV-07 classifier gap: added a private `is_transport_failure(&str) -> bool` helper to `AgentLoop` beside `extract_http_status` (conservative 7-needle case-insensitive allowlist over `format!("{err:#}")` — `connection refused`, connect timeout, DNS failure, connection reset, the reqwest `error sending request for url` marker), and changed `classify_llm_error`'s `None`-status branch from `(true, false)` to `(true, Self::is_transport_failure(&err_str))` so the already-wired `fallback_providers` chain actually activates when the request never reaches the provider; `run()` untouched, `should_retry` stays `true` so the no-fallback path is behaviorally unchanged. Repurposed `test_classify_other_error`, added 6 unit tests (5 transport cases + an `SSE stream read timed out` Pitfall-1 guard), and added `crates/ironhermes-agent/tests/invariants_27_1_4_1_1.rs` static-grep gates. 11/11 must-haves verified; live Ollama-down→OpenRouter smoke test passed (user). Code review: 0 blockers, 3 advisory warnings (allowlist-only detection; response-body false-positive risk; reliance on `extract_http_status` returning `None`) — accepted as the deliberate D-01 conservative-classifier tradeoff.*

*Earlier: 2026-05-12 — Phase 27.1.4.1 (gateway-fallback-gap) complete. Closed the PROV-07 coverage gap: extracted `wire_fallback_if_configured(agent, &resolver) -> AgentLoop` in `ironhermes-agent` (re-exported from `lib.rs`, two `tracing::warn!` branches), wired it at the 3 production `AgentLoop` sites that previously failed hard with no retry (gateway cron runner, agent subagent runner, CLI batch runner), and refactored the 2 existing silent inline-`if let` fallback sites (`handler.rs`, `iron_hermes_ui/server/state.rs`) onto the same helper. Added 3 static-invariant test files; documented the `learning:` and `tools:` sections in `cli-config.yaml.example` and the `fallback_providers` config + Ollama use case in README. 12/12 must-haves verified.*

*Earlier: 2026-05-14 — Phase 27.1.4.2 (hexapod-led-off-fails) complete. Fixed two bugs in `ironhermes-tools`: (1) `CMD_LED_OFF` constant corrected from `"CMD_LED#0\n"` (color channel, silently ignored by Freenove server) to `"CMD_LED_MOD#0\n"` (mode channel, triggers `color_wipe([0,0,0])`), restoring physical LED off behavior on the robot; (2) cross-module `ENV_LOCK` race eliminated by adding a single `pub(crate) static ENV_LOCK` to `lib.rs` under `#[cfg(test)]` and swapping both `hexapod_tcp::tests` and `hexapod_video::tests` onto it via `use crate::ENV_LOCK` — ends flaky `test_capture_frame_passes_allowlist` in parallel mode. Added `crates/ironhermes-tools/tests/invariants_27_1_4_2.rs` static-grep regression gate (positive + negative `CMD_LED_MOD#0`/`CMD_LED#0` assertions citing PROV-HEXAPOD). 341 lib tests + 2 invariant tests pass in default parallel mode; 3/3 must-haves verified.*

*Earlier: 2026-05-12 — Phase 27.1.3 (Expression + Skill Doc) complete. Added LED control to `hexapod_tcp` (led/led_off, CMD_LED/CMD_LED_OFF, RGB clamped 0–255, 26 unit tests pass). Created `skills/hexapod/SKILL.md` — protocol-complete reference for all 12 actions, auto-activates with `requires_toolsets: [robotics]`. Requirements HXP-NAV-02, HXP-DOC-01 satisfied; 10/10 must-haves verified. Hexapod tool is now feature-complete (12 actions, full protocol documentation).*
