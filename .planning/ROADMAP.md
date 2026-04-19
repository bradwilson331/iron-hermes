# IronHermes Roadmap

> Phase-indexed roadmap. Each phase links to a phase directory under `.planning/phases/`.

---

## Active

### Phase 20: Memory Provider Plugin Contract

**Status:** Planned
**Goal:** Bring the Rust `MemoryProvider` trait to API parity with the hermes-agent Python plugin contract (enriched hook surface, `ConfigField` schema, `MemoryManager` layer with write-only mirror) — without introducing runtime plugin discovery, per PROJECT.md:52. Migrate `initialize` signature (breaking) across all three external provider crates. Fold in two pending todos: factory `load_from_disk` regression (Fix 1) and chat-mode memory wiring (Fix 2).

**Requirements:** MEM-07, MEM-08, MEM-09, MEM-10, MEM-11, MEM-12

**Plans:** 4/4 plans complete

Plans:
- [x] 20-01-trait-enrichment-and-factory-fix-PLAN.md — Enrich MemoryProvider trait (defaulted new hooks + required `name()`), introduce `ConfigField`/`MemoryAction` in `ironhermes-core/src/config_schema.rs`, delete `MemoryProviderConfig`, migrate all three provider crates + file `MemoryStore` to new `initialize(session_id, hermes_home, &Value)` signature, make factory async with `load_from_disk` for every provider + `is_available` fallback, round-trip regression test (Fix 1)
- [x] 20-02-memory-manager-and-wiring-PLAN.md — Create `crates/ironhermes-agent/src/memory/manager.rs` (`MemoryManager` wrapping primary + optional write-only mirror, 5s timeout, swallow-on-error, reserved-name guard), rewire `MemoryTool` / `agent_loop.queue_prefetch` / `context_engine.on_pre_compress` / `prompt_builder.system_prompt_block`, add hook-ordering contract test
- [x] 20-03-setup-wizard-and-chat-wiring-PLAN.md — `hermes memory setup` CLI wizard (minimal per D-08; POSIX-safe .env appends, deny-list, `RedactedValue`), wire `MemoryManager` into `run_chat` / `run_single` (Fix 2), cross-invocation persistence regression test
- [x] 20-04-provider-hook-adoption-PLAN.md — Each provider (file/sqlite/duckdb/grafeo) overrides `name()` + `get_config_schema()` with real fields; per-provider config-schema unit tests; sqlite mirror fixture proving `on_memory_write` end-to-end through MemoryManager

**Wave structure:**
- Wave 1: 20-01 (trait + factory + provider migration — autonomous)
- Wave 2: 20-02 (MemoryManager + wiring — depends on 20-01, autonomous)
- Wave 3: 20-03 and 20-04 in parallel (depends on 20-02, both autonomous)

**Phase directory:** `.planning/phases/20-memory-provider-plugin-contract/`

### Phase 22: CLI Tool Parity

**Goal:** Wire execute_code, skills_tool, cron_tool, BlocklistGuardrail, and HookRegistry (JSONL event logging + webhook listeners) into both `run_chat` and `run_single` CLI paths, achieving full tool-level parity with `run_gateway`. Pass the HookRegistry to AgentLoop and attach_context_engine so all lifecycle events fire in CLI mode. Per D-01: this phase covers CLI-01 only (tool parity). TUI extension hooks split to Phase 22.1; ACP adapter split to Phase 22.2.

**Requirements:** CLI-01
**Depends on:** Phase 21
**Plans:** 2/2 plans complete

Plans:
- [x] 22-01-PLAN.md — Wire cron_tool, skills_tool, execute_code_tool (with shared active_skills Arc and D-04 safe-subset RPC registry), and BlocklistGuardrail + error_detail into both run_chat and run_single, matching run_gateway's tool registration sequence per D-08.
- [x] 22-02-PLAN.md — Construct HookRegistry with JSONL listener (D-06) and webhook listeners (D-07) in both CLI paths. Wire hook_registry into run_agent_turn (AgentLoop builder) and attach_context_engine (D-09). Drain retry queue on startup. Add static-grep regression tests for all wiring calls.

**Wave structure:**
- Wave 1: 22-01 (tool registration parity — autonomous)
- Wave 2: 22-02 (HookRegistry wiring + regression tests — depends on 22-01, autonomous)

**Phase directory:** `.planning/phases/22-cli-feature-parity/`

### Phase 22.1: TUI Extension Hooks

**Goal:** Create a Rust extension mechanism for the CLI TUI so that external code (plugins, custom builds, future crates) can add widgets, keybindings, layout sections, command handlers, and style overrides -- the Rust equivalent of hermes-agent's subclassable CliManager. Implements a hybrid three-layer architecture: TuiExtension trait (static contract), mpsc message bus (dynamic updates), and command registry (extension-first dispatch). Slot-based layout with dynamic DECSTBM scroll region adjustment. No new dependencies.
**Requirements:** CLI-02
**Depends on:** Phase 22
**Plans:** 2/2 plans complete

Plans:
- [x] 22.1-01-PLAN.md — Define pure-function type contracts: TuiExtension trait, Widget/LayoutSlot/TuiEvent types, KeybindingRegistry, CommandRegistry with extension-first dispatch chain and unit tests
- [x] 22.1-02-PLAN.md — Wire extension contracts into render loop (dynamic DECSTBM, widget slot compositing, TuiEvent channel) and REPL loop (pre-readline keybinding dispatch, extension-first command routing)

**Wave structure:**
- Wave 1: 22.1-01 (pure types + trait + registries — autonomous)
- Wave 2: 22.1-02 (render.rs + main.rs integration — depends on 22.1-01, autonomous)

**Phase directory:** `.planning/phases/22.1-tui-extension-hooks/`

### Phase 22.2: ACP Adapter

**Goal:** [To be planned]
**Requirements:** CLI-03, CLI-04, CLI-05, CLI-06, CLI-07, CLI-08
**Depends on:** Phase 22
**Plans:** 0 plans

Plans:
- [ ] TBD (run /gsd-plan-phase 22.2 to break down)

---

### Phase 21: Commandline UI update — polish CLI UX including graceful double ctrl-c handling in agent mode (first interrupt cancels in-flight turn/stream and returns to prompt; second exits cleanly)

**Goal:** Polish `crates/ironhermes-cli/` REPL UX on existing deps (crossterm/rustyline/colored/tokio — no new crates per D-18): render a persistent dot-separated pill status line at the bottom (mode · model · provider · tokens/limit · hint, alternating cyan/magenta/green/yellow/dimmed), animate a 10-cell Knight Rider scanner during in-flight turns/tools, and implement graceful double ctrl-c where the first press cancels the in-flight turn (preserving conversation history) and the second press within 1.5s persists the session as "interrupted" and exits cleanly. Rolls in todo (2026-04-13). CONTEXT.md decisions D-01..D-22 serve as requirements for this phase (no REQ-IDs map).

**Requirements:** (none — D-01..D-22 from 21-CONTEXT.md are the requirements)
**Depends on:** Phase 20
**Plans:** 3/3 plans complete

Plans:
- [x] 21-01-tui-scaffold-and-pure-cores-PLAN.md — Scaffold `crates/ironhermes-cli/src/tui/` module tree (mod.rs, activity.rs, pills.rs, knight_rider.rs, double_ctrl_c.rs, status_line.rs). Implement all pure-function cores with full unit tests: pill color rotation (D-04), knight-rider triangle-wave frame generator (D-06/D-07), double-ctrl-c state machine (D-10..D-14), status-line pure renderer (D-03/D-05). No main.rs wiring yet — zero runtime behavior change.
- [x] 21-02-activity-watch-and-render-task-PLAN.md — Build the rendering I/O layer: `TuiHandle` owning two `tokio::sync::watch` channels (ActivityState + StatusLineState) and a 100ms-tick render task that writes to stderr via crossterm absolute cursor positioning with Hide/Show flicker guards (D-15/D-16/D-17). Auto-detects non-tty stderr and no-ops (Open Q5). Re-queries `size()` each tick for SIGWINCH tolerance. Not yet wired into main.rs.
- [x] 21-03-run-chat-integration-and-double-ctrl-c-PLAN.md — Wire TuiHandle into `run_chat` (streaming + tool-progress callbacks publish ActivityState; remove old `\r Running: …` clutter per D-08). Install `tokio::signal::ctrl_c` in a `tokio::select!` around the agent future (D-10). Parent CancellationToken lives the session; per-turn children via `.child_token()` (RESEARCH §Pitfall 2). Wire DoubleCtrlCState (D-11, D-12, D-13). Preserve rustyline-Interrupted branch (D-14). 3rd-ctrl-c-within-3s emergency escape (RESEARCH §Pitfall 7). Static-grep regression tests for INV-1..INV-6. Manual VALIDATION.md walkthrough (D-22). Move rolled-in todo to completed/.

**Wave structure:**
- Wave 1: 21-01 (pure-function cores — autonomous)
- Wave 2: 21-02 (TuiHandle + render task — depends on 21-01, autonomous)
- Wave 3: 21-03 (main.rs integration + manual QA — depends on 21-01 and 21-02, NOT fully autonomous: final task is `checkpoint:human-verify`)

**Rolls in todo:** [cli] Double ctrl-c in agent mode ends process and thread (2026-04-13) — see `.planning/todos/pending/2026-04-13-double-ctrl-c-in-agent-mode-ends-process-and-thread.md`

**Phase directory:** `.planning/phases/21-commandline-ui-update-polish-cli-ux-including-graceful-doubl/`

### Phase 21.3: Model metadata & models.dev — context lengths, token estimation (INSERTED)

**Goal:** Replace the hardcoded `DEFAULT_CONTEXT_LENGTH = 128_000` with a model-aware metadata system (static lookup table + disk cache from models.dev/OpenRouter APIs), replace the crude `text.len() / 4 + 1` token estimation heuristic with proper BPE tokenization via tiktoken-rs, wire accurate model metadata through all consumers (AgentLoop, ContextCompressor, PressureTracker, StatusLine), and add `hermes models list/fetch/info` CLI subcommand plus `/models` slash command. D-01..D-15 from CONTEXT.md serve as requirements.
**Requirements:** D-01, D-02, D-03, D-04, D-05, D-06, D-07, D-08, D-09, D-10, D-11, D-12, D-13, D-14, D-15
**Depends on:** Phase 21
**Plans:** 2/4 plans executed

Plans:
- [x] 21.3-01-PLAN.md — Create ModelMetadata/ModelCapabilities/ModelRegistry structs with static lookup table (30+ models across 7 families), canonical ID + alias map (versioned/prefixed/legacy name resolution), TokenEstimator wrapping tiktoken-rs singletons (cl100k_base + o200k_base), global estimator with OnceLock, warm function. All in ironhermes-core with comprehensive TDD unit tests.
- [x] 21.3-02-PLAN.md — Wire metadata through resolution chain: add model_metadata to ResolvedEndpoint, populate from ModelRegistry in ProviderResolver::build(), replace text.len()/4 heuristic with tiktoken in context_compressor, parameterize attach_context_engine with context_length, update all four hardcoded 128_000 sites in main.rs and gateway handler.
- [ ] 21.3-03-PLAN.md — Implement disk cache (ModelsCache with load/save to ~/.ironhermes/models-cache.json) and API fetch layer (models.dev primary + OpenRouter fallback per D-03), parse functions for both API response formats, normalize_model_id, fetch_all with fallback chain and FetchResult reporting.
- [ ] 21.3-04-PLAN.md — Add hermes models list/fetch/info CLI subcommands (models_cmd.rs following cron.rs pattern, UI-SPEC terminal output contracts) and /models refresh|info slash commands (plain text CommandResult::Output, no ANSI codes). Wire into Commands enum and CommandRouter registry.

**Wave structure:**
- Wave 1: 21.3-01 (ModelMetadata + TokenEstimator + static table — autonomous)
- Wave 2: 21.3-02 and 21.3-03 in parallel (wiring + cache/fetch — both depend on 01, both autonomous)
- Wave 3: 21.3-04 (CLI subcommand + slash command — depends on 02 and 03, autonomous)

**Phase directory:** `.planning/phases/21.3-model-metadata-models-dev-context-lengths-token-estimation/`

### Phase 21.2: MCP client tool and fold in slash commands related to MCP client use (INSERTED)

**Goal:** Port hermes-agent's MCP client infrastructure to IronHermes: new `ironhermes-mcp` crate using the official `rmcp` SDK for stdio and HTTP/StreamableHTTP transports, per-server tokio tasks with exponential backoff reconnection, tool discovery and registration into a dynamically-mutable `Arc<RwLock<ToolRegistry>>`, sampling support, credential stripping, safe env filtering, `/reload-mcp` slash command, and `hermes mcp add/remove/list/test/configure` CLI subcommands. D-01..D-21 from CONTEXT.md serve as requirements.
**Requirements:** D-01, D-02, D-03, D-04, D-05, D-06, D-07, D-08, D-09, D-10, D-11, D-12, D-13, D-14, D-15, D-16, D-17, D-18, D-19, D-20, D-21
**Depends on:** Phase 21
**Plans:** 5 plans

Plans:
- [ ] 21.2-01-PLAN.md — Create ironhermes-mcp crate scaffold with rmcp dependency, McpServerConfig (hermes-agent-compatible YAML schema), env var interpolation, build_safe_env allowlist, sanitize_error credential stripping, and mcp_servers field on Config struct
- [ ] 21.2-02-PLAN.md — Add register_dynamic/unregister_by_prefix to ToolRegistry and migrate all Arc<ToolRegistry> callsites to Arc<RwLock<ToolRegistry>> across 6 files (rpc_registry stays Arc<ToolRegistry> for safe subset isolation)
- [ ] 21.2-03-PLAN.md — Implement McpManager orchestrating per-server tokio tasks, McpTool (Tool trait impl with channel-based dispatch), stdio/HTTP transport helpers via rmcp SDK, sampling handler with rate limiting, exponential backoff reconnection (5 retries, max 60s), tool naming (server__tool), description prefixing ([MCP: server_name]), enabled_tools filtering, and reload capability
- [ ] 21.2-04-PLAN.md — Add McpReloader trait to CommandContext (circular-dep resolution), wire /reload-mcp and /reload handlers replacing todo stubs, integrate McpManager into run_chat/run_single/run_gateway with background discovery, handle McpReload CommandResult in REPL loop
- [ ] 21.2-05-PLAN.md — Implement hermes mcp add/remove/list/test/configure CLI subcommands in mcp_config.rs with interactive wizard, config.yaml persistence, test connection via rmcp, and UI-SPEC styled output (colored crate matching cron.rs patterns)

**Wave structure:**
- Wave 1: 21.2-01 and 21.2-02 in parallel (crate scaffold + registry migration — both autonomous)
- Wave 2: 21.2-03 (McpManager + server tasks + McpTool — depends on 01 and 02, autonomous)
- Wave 3: 21.2-04 and 21.2-05 in parallel (slash command wiring + CLI subcommands — both depend on 03, both autonomous)

**Phase directory:** `.planning/phases/21.2-mcp-client-tool-and-fold-in-slash-commands-related-to-mcp-cl/`

### Phase 21.1: Slash Commands (INSERTED)

**Goal:** Implement platform-agnostic slash command router that intercepts `/` prefixed messages before AgentLoop dispatch, with full hermes-agent command parity (44 commands), alias resolution, shortest-unique-prefix matching, platform availability filtering, and running-agent guard. Replace hardcoded CLI and gateway dispatchers with unified router in ironhermes-core. Works across CLI, gateway, and ACP.
**Requirements:** SKILL-12, SKILL-13, SKILL-14
**Depends on:** Phase 21
**Plans:** 2/2 plans complete

Plans:
- [x] 21.1-01-PLAN.md — Build CommandRouter in ironhermes-core: CommandDef/PlatformFilter/CommandCategory types, three-stage resolve_command (exact/alias/prefix), full 44-command registry with wired handlers and TODO stubs, CommandContext struct, comprehensive unit tests
- [x] 21.1-02-PLAN.md — Wire CommandRouter into CLI (replace core_dispatch, update dispatch_command chain, construct CommandContext in REPL loop) and gateway (replace handle_slash_command with router-based dispatch, delete old cmd_* methods). Static-grep regression tests.

**Wave structure:**
- Wave 1: 21.1-01 (core router + registry + handlers + tests — autonomous)
- Wave 2: 21.1-02 (CLI + gateway integration — depends on 21.1-01, autonomous)

**Phase directory:** `.planning/phases/21.1-slash-commands/`
