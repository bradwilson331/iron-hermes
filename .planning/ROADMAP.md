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

### Phase 22.4: ratatui-backed REPL (tmon architecture) (INSERTED)

**Goal:** Replace `hermes chat`'s custom crossterm + rustyline + raw-ANSI REPL (~3,126 LOC in `crates/ironhermes-cli/src/tui/`) with a ratatui-driven REPL modelled after the `tmon` reference architecture at `/Users/twilson/code/tmon/`. Lands as a side-by-side `tui_rata/` module that defaults on for interactive TTY sessions while preserving the classic path as an explicit `--classic-tui` opt-out + `IRONHERMES_CLASSIC_TUI=1` env var + `IsTerminal` non-TTY fall-back for one cycle (D-02, D-03, D-04). Full feature parity with the existing `run_chat` wiring: AgentLoop streaming, HookRegistry (JSONL + webhook), MCP manager + `/reload-mcp`, memory_manager + MemoryTool, SubagentRegistry + TranscriptWriter, ProcessRegistry + `/agents` + `/stop`, 49-command slash router, typo suggester, BlocklistGuardrail, cron/skills/execute_code tools, `--yolo` gate + banner, CancellationToken cascade, double-Ctrl-C state machine, status pills, knight-rider scanner — 14-item parity locked by D-18. Workspace crossterm bump 0.28 → 0.29 (D-13). New deps: ratatui 0.30, tui-textarea 0.7, ansi-to-tui 8, tui-logger 0.18 (D-14). Dual-layer testing: 23-row INV-22.4-* static-grep regression suite + 8-frame ratatui TestBackend + insta snapshot suite (D-19). 19 CONTEXT decisions (D-01..D-19) serve as the requirements set — no new REQ-IDs map (Phase 21 / 22.3 precedent).

**Requirements:** (none — D-01..D-19 from 22.4-CONTEXT.md are the requirements)

**Depends on:** Phase 22, Phase 22.1 (TuiExtension retired in tui_rata/ per D-09 but trait kept exported for classic-tui), Phase 22.3 (`$HERMES_HOME/repl_history` contract + D-08 codec reuse)

**Plans:** 13/13 plans complete

Plans:
- [x] 22.4-00-PLAN.md — Wave 0: Workspace dep floor bump (crossterm 0.28 → 0.29 with event-stream feature; ratatui 0.30, tui-textarea 0.7, ansi-to-tui 8, tui-logger 0.18 workspace + crate deps) + cargo-tree spike confirming single ratatui compile unit (Pitfall §1). Checkpoint if spike fails → tui-textarea-2 fallback approval.
- [x] 22.4-01-PLAN.md — Wave 1: tui_rata/ scaffold + verbatim-lift pure cores (knight_rider.rs + double_ctrl_c.rs); create lib.rs target so integration tests can `use ironhermes_cli::tui_rata::*`.
- [x] 22.4-02-PLAN.md — Wave 2: port `tui/keybindings.rs` → `tui_rata/keybindings.rs` with TuiExtension dep surgically removed per D-09 (widget-slot system retired in tui_rata/).
- [x] 22.4-03-PLAN.md — Wave 3: port `tui/status_line.rs` + `tui/pills.rs` → `tui_rata/status_line.rs`; swap `colored::ColoredString` output for ratatui `Line<'static>` of styled `Span`s; pill palette Cyan/Magenta/Green/Yellow locked by regression test.
- [x] 22.4-04-PLAN.md — Wave 4: `tui_rata/stream_events.rs` (D-17 canonical 8-variant enum — Started, Delta, ToolCall, ToolProgress, ToolResult, Finished, Error, Cancelled) + `tui_rata/history.rs` (ReplHistory with U+001F unit-separator codec per D-08, 1000-entry cap per D-07, dedupe-consecutive, rustyline-compatible load/save).
- [x] 22.4-05-PLAN.md — Wave 5: `tui_rata/app.rs` — central App struct with all 14 D-18 parity-list fields (AgentLoop/HookRegistry/McpManager/MemoryManager/SubagentRegistry/ProcessRegistry/CommandRouter Arc handles + CancellationToken cascade + StatusLineState + knight_rider_tick + DoubleCtrlCState + ReplHistory + yolo_enabled). Verbatim scroll math from tmon (scroll_up/scroll_down/scroll_indicator/reconcile_scroll/transcript_max_scroll/transcript_line_count/wrapped_line_count). Event handlers (handle_key with D-06 Up/Down=history-recall + D-08 Enter=submit + KeyEventKind::Press filter per Pitfall 7; handle_mouse; handle_stream_event 8-variant match; handle_ctrl_c_key / handle_ctrl_c_signal; on_tick; submit stubs channel + cancel_child). Test-only constructors `App::new_test_empty()` + `App::new_test_with_messages()` gated on `test-support` feature for plan 22.4-10 snapshots.
- [x] 22.4-06-PLAN.md — Wave 6: `tui_rata/ui.rs` pure frame render — 4-chunk Vertical Layout (Min(5) transcript + Length(1) knight-rider + Length(1) status pills + Length(3) tui-textarea input per CONTEXT §specifics); transcript Paragraph with scroll((transcript_scroll, 0)) + Wrap{trim:false} + bordered block titled `Chat [{scroll_indicator}]`; knight-rider via `ansi_to_tui::IntoText`; status pills via `render_status_line_ratatui`; cursor via `frame.set_cursor_position`.
- [x] 22.4-07-PLAN.md — Wave 7: `tui_rata/event_loop.rs` + `run_chat_ratatui` async entry point. `ratatui::init()` / `ratatui::restore()` D-15 primary path + RAII MouseCaptureGuard per D-01. `TuiTracingSubscriberLayer` installed before `ratatui::init()` per Pitfall 2. 14-item D-18 parity wiring ported from classic `main.rs::run_chat` (lines 669–1800) preserving registration order. 4-arm `tokio::select!` main loop over EventStream + pending_rx + ctrl_c (pinned once per Pitfall 6) + 100ms tick per D-16 canonical shape. Per-turn `tokio::spawn` bridge with `UnboundedSender<StreamEvent>` per D-17. SIGWINCH tolerance via per-iteration `terminal.size()?` re-query. EventStream local to event_loop function per Pitfall 10.
- [x] 22.4-08-PLAN.md — Wave 8: `main.rs` integration — add `classic_tui: bool` to Cli struct with `#[arg(long = "classic-tui")]`; `should_use_classic_tui(cli)` helper implementing D-03/D-04 precedence (CLI flag > env var > non-TTY IsTerminal gate). Replace `Commands::Chat` arm body + bare-hermes arm body with four-way branch routing to `tui_rata::run_chat_ratatui` vs classic `run_chat`. Gate the existing `tracing_subscriber::fmt().init()` so ratatui-for-chat path defers to `run_chat_ratatui` (Pitfall 2). `print_yolo_banner_to_stderr` fires pre-alt-screen in ratatui branch. `run_single` + `run_gateway` UNTOUCHED (D-02 + 22.3 D-15 run_chat-only precedent).
- [x] 22.4-09-PLAN.md — Wave 9: `crates/ironhermes-cli/tests/invariants_22_4.rs` — 23 static-grep regression gates INV-22.4-01..23 per D-19 Layer 1 + RESEARCH §INV-22.4 anchor table + PATTERNS.md 23-row map. Locks all 14 D-18 parity-list wiring call sites + structural invariants (ratatui init/restore pair, EventStream local to event_loop, KeyEventKind::Press filter, mouse-capture pair, unit-separator codec, CancellationToken cascade ≥ 2 child tokens). Sibling file pattern following invariants_22_3_streaming.rs. Zero new dev-deps.
- [x] 22.4-10-PLAN.md — Wave 9: `crates/ironhermes-cli/tests/tui_rata_snapshots.rs` — 8 canonical-frame ratatui `TestBackend` + `insta` snapshot tests per D-19 Layer 2: empty transcript, 2-message conversation, in-flight streaming partial delta, tool-call activity row, scroll-active indicator, double-Ctrl-C pending-exit warning, error banner, 3-line multi-line input. Gated on `test-support` feature. Checkpoint for operator `cargo insta review` + visual verification before snapshot acceptance.
- [x] 22.4-11-PLAN.md — Wave 10 (gap closure, D-03): insert `print_banner(); io::stdout().flush().ok(); io::stderr().flush().ok();` in BOTH ratatui dispatch arms in main.rs (Commands::Chat arm + bare-hermes arm), mirroring the classic run_chat line 758-764 GAP-5 pattern. Add INV-22.4-25 to static-grep-lock print_banner co-occurrence with run_chat_ratatui at the dispatch layer. Closes VERIFICATION.md Gap 1.
- [x] 22.4-12-PLAN.md — Wave 10 (gap closure, D-17 / CR-02): wire `AgentLoop::with_tool_progress(...)` + new `AgentLoop::with_tool_result(...)` builder on per-turn AgentLoop in `spawn_turn` so all 8 D-17 canonical StreamEvent variants (adding ToolCall, ToolProgress, ToolResult) are emitted from production — not just from direct-inject snapshot tests. Adds a small symmetric `ToolResultCallback = Box<dyn Fn(&str, bool) + Send + Sync>` type to agent_loop.rs + 6 callback-firing sites at existing `fire_hook(HookEventKind::ToolCompleted { success, .. })` locations. Adds INV-22.4-26 (with_tool_progress / with_tool_result chained) + INV-22.4-27 (all 3 variants constructed in event_loop.rs). Closes VERIFICATION.md Gap 2 + REVIEW.md CR-02.

**Wave structure:**
- Wave 0: 22.4-00 (workspace dep floor + spike checkpoint — autonomous except checkpoint)
- Wave 1: 22.4-01 (tui_rata/ scaffold + pure-core lifts — autonomous)
- Wave 2: 22.4-02 (keybindings port — depends on 01, autonomous)
- Wave 3: 22.4-03 (status_line port — depends on 02, autonomous)
- Wave 4: 22.4-04 (stream_events + history — depends on 03, autonomous)
- Wave 5: 22.4-05 (App struct + scroll math + handlers — depends on 04, autonomous)
- Wave 6: 22.4-06 (ui.rs frame render — depends on 05, autonomous)
- Wave 7: 22.4-07 (event_loop + run_chat_ratatui — depends on 06, autonomous)
- Wave 8: 22.4-08 (main.rs Commands::Chat dispatch — depends on 07, autonomous)
- Wave 9 parallel: 22.4-09 (INV regression tests — depends on 08, autonomous) + 22.4-10 (snapshot tests — depends on 08, NOT autonomous — human insta review checkpoint)
- Wave 10 parallel (gap closure, depends on Wave 9 — all earlier plans already executed): 22.4-11 (D-03 print_banner in ratatui dispatch arms + INV-22.4-25; touches main.rs + invariants_22_4.rs) + 22.4-12 (D-17 / CR-02 tool-progress + tool-result wiring + INV-22.4-26/27; touches agent_loop.rs + event_loop.rs + invariants_22_4.rs). Both plans touch invariants_22_4.rs so in strict parallel mode they contend — the executor MUST sequentialise the tests/invariants_22_4.rs writes (append-only, no reordering) or run the two plans serially. Source file modifications are disjoint (main.rs vs agent_loop.rs+event_loop.rs).

Waves 2–8 are serialised because each plan extends `tui_rata/mod.rs`; file-ownership conflicts force a linear chain. Waves 0–1, 9, and 10 are the genuinely-parallel opportunities (subject to the invariants_22_4.rs append-only constraint in Wave 10).

**Live-TTY HUMAN-UAT:** Per CONTEXT D-19 Layer 3, after all 11 plans land the operator re-runs the 3-concurrent-subagent LoRA-research scenario from `22.3-UAT-EVIDENCE.md` against `tui_rata/` and records pass/fail in `22.4-HUMAN-UAT.md`. Gates the follow-up phase (22.5) that deletes classic-tui.

**Phase directory:** `.planning/phases/22.4-ratatui-backed-repl-tmon-architecture/`

### Phase 22.4.2: wire up slash commands (INSERTED)

**Goal:** [Urgent work - to be planned]
**Requirements**: TBD
**Depends on:** Phase 22.4
**Plans:** 5/5 plans complete

Plans:
- [x] TBD (run /gsd-plan-phase 22.4.2 to break down) (completed 2026-04-26)

### Phase 22.4.2.3: fix the pre-existing INV-22.3-02 banner-bleed before milestone (INSERTED)

**Goal:** Update the static-grep regression test `invariant_22_3_02_banner_called_exactly_once_before_tui_init` in `crates/ironhermes-cli/tests/invariants_22_3.rs` so it accepts the legitimate Phase 22.4 ratatui-dispatch additions (Plan 22.4-11, commit `f1aeb73`) without losing regression intent. Replaces the stale `count == 1` equality with `count >= 1`, strengthens the ordering check from "first call site before TUI init" to "every call site strictly before `TuiHandle::new_with_extensions`", anchors on the qualified `TuiHandle::new_with_extensions` string, renames the test to `invariant_22_3_02_banner_called_at_least_once_strictly_before_tui_init`, and rewrites the doc-comment + assertion messages to cite Phase 22.4 CONTEXT D-03 as the rationale for accepting more than one site. Test-only change — `crates/ironhermes-cli/src/main.rs` is untouched. CONTEXT decisions D-01..D-06 in `22.4.2.3-CONTEXT.md` serve as the requirements set (no REQ-IDs).
**Requirements:** (none — D-01..D-06 from 22.4.2.3-CONTEXT.md serve as the requirements set)
**Depends on:** Phase 22.4.2
**Plans:** 1/1 plans complete

Plans:
- [x] 22.4.2.3-01-PLAN.md — Rewrite + rename `invariant_22_3_02_banner_called_exactly_once_before_tui_init` to relaxed-count (`>= 1`) + every-position-before-`TuiHandle::new_with_extensions` form per CONTEXT D-01..D-06; test-only edit, main.rs untouched.

### Phase 22.4.2.2: Cron create defaults to TG origin when gateway active (INSERTED)

**Goal:** Restore the v1.x ergonomic where `hermes cron create` (and the LLM `cronjob` tool) auto-route a new job back to the configured Telegram chat when the gateway has exactly one authorized chat. Plan 01 adds `OriginDecision` enum + `Config::telegram_default_origin()` helper to `ironhermes-core::config` and consults it from `cmd_create` (CLI path); Plan 02 consults the same helper from `cronjob_tool::handle_create` (LLM-tool path) using `tracing::warn` (not `eprintln`) for the multi-chat hint to avoid polluting LLM tool output. Existing jobs are NOT migrated (D-12). Helper bypassed when explicit `--deliver` flag / `deliver` arg is provided (D-04). Final INV ledger: 64 INVs in `invariants_22_4.rs` (62 + 1 per plan).
**Requirements:** (none — D-01..D-12 from 22.4.2.2-CONTEXT.md serve as the requirements set)
**Depends on:** Phase 22.4.2.1
**Plans:** 2/2 plans complete

Plans:
- [x] 22.4.2.2-01-PLAN.md — CLI default-routing: add `OriginDecision` enum + `Config::telegram_default_origin()` to `ironhermes-core::config`; flip `Create.deliver` to `Option<String>` and consult the helper from `cmd_create` via new `pub(crate) fn resolve_cron_deliver`; 5 behavioral tests in new `cron_default_deliver.rs` + 4 unit tests in `config.rs::mod tests` + INV-22.4.2.2-01 (function 63)
- [x] 22.4.2.2-02-PLAN.md — LLM tool default-routing: consult Plan 01's helper from `cronjob_tool::handle_create` (lazy `Config::load()` inside the handler) using `tracing::warn!` for the multi-chat hint; 5 behavioral tests in new `cronjob_tool_default_deliver.rs` mirroring Plan 01 + INV-22.4.2.2-02 (function 64); pure addition — `CronjobTool::new` signature, `register_cronjob_tool`, and main.rs / tui_rata callsites all unchanged

**Wave structure:**
- Wave 1: 22.4.2.2-01 (helper + CLI consumer + 5 tests + INV-01 — autonomous)
- Wave 2: 22.4.2.2-02 (LLM-tool consumer + 5 tests + INV-02 — depends on 22.4.2.2-01 because it consults the helper added by Plan 01; autonomous)

### Phase 22.4.2.1: Cron cmds and telegram delivery broken (INSERTED)

**Goal:** [Urgent work - to be planned]
**Requirements**: TBD
**Depends on:** Phase 22.4.2
**Plans:** 3/3 plans complete

Plans:
- [x] TBD (run /gsd-plan-phase 22.4.2.1 to break down) (completed 2026-04-26)

### Phase 22.4.1: tui_rata handler re-port — route dispatch_slash through CommandRouter and registry handlers (INSERTED)

**Goal:** Re-port `crates/ironhermes-cli/src/tui_rata/commands.rs::dispatch_slash` so every visible-surface slash command resolves through `ironhermes_core::commands::CommandRouter`, retiring the four ad-hoc `strip_prefix` fast-paths added in Plans 22.4-16 (`/mouse`) and 22.4-18 (`/mcp`, `/sessions`, `/memory`). After this phase `dispatch_slash`'s shape is symmetric with `tui::commands::dispatch` (classic-tui) and `gateway::handler::handle_slash_command` — pure router-shell + one localised post-router App-side hook for `/mouse`'s state-mutation. Promote `mouse`/`mcp`/`sessions`/`memory` into `ironhermes-core::commands::registry::build_registry()` (4 new `CommandDef::new` entries). Bulk-fill `invoke_handler` with explicit `"<name>" => CommandResult::Output(...)` arms for every Platform::Local-reachable command in the Session and Configuration registry categories (~26 net-new arms following the locked D-08 stub-text format with `Phase 22.4.1 stub:` markers). Replace the 22-line hand-built `render_help()` with a router-driven `render_help_router(router, &Platform::Local)` lifted from `tui::commands::format_help`'s pure-text inner loop (D-13). Test ledger continues from {00..22, 24..31} = 31 invariants to {00..22, 24..34} = 34 invariants — INV-22.4-29 + INV-22.4-31 inverted in place per the 22.4-16/17/18 numbering precedent; INV-22.4-32 (router_membership), INV-22.4-33 (invoke_handler_arms), INV-22.4-34 (dispatch_slash_no_strip_prefix) appended. Pure refactor — no new behavior, snapshot suite predicted zero diffs (none of the 8 canonical frames render `/help` output). 15 locked CONTEXT decisions D-01..D-15 serve as the requirements set.

**Requirements:** (none — D-01..D-15 from 22.4.1-CONTEXT.md are the requirements)
**Depends on:** Phase 22.4
**Plans:** 3/3 plans complete

Plans:
- [x] 22.4.1-00-PLAN.md — Wave 1: core registry — register 4 new CommandDefs (`mouse`/Configuration/CliOnly, `mcp`/ToolsAndSkills/Universal, `sessions`/Session/Universal, `memory`/ToolsAndSkills/Universal) in `crates/ironhermes-core/src/commands/registry.rs` + add INV-22.4-32 (router_membership) to `tests/invariants_22_4.rs` with new `CORE_REGISTRY` const. Behavior-neutral — tui_rata fast-paths still fire BEFORE the router until Plan 01 retires them. Implements D-01, D-05 (registry portion), D-09, D-14.
- [x] 22.4.1-01-PLAN.md — Wave 2: tui_rata refactor — retire 4 `strip_prefix` fast-paths in `dispatch_slash`; widen `invoke_handler` signature to `(name, _ctx, router)`; add post-router App-side hook `if def.name == "mouse"` calling `handle_mouse_slash(app, args)` with `def.name`-interpolated args extraction (D-10/D-11/D-12); add 4 new invoke_handler arms (mouse/mcp/sessions/memory) per D-08 stub format; replace hand-built `render_help` with private `render_help_router(router, &Platform::Local)` lifted from `tui::commands::format_help` inner loop (D-13); delete 3 dead helpers `handle_mcp_slash`/`handle_sessions_slash`/`handle_memory_slash` (RESEARCH Finding 7); preserve `handle_mouse_slash` + `/agents` + `/skills` arms + generic `not yet wired` fallback verbatim (D-06/D-07); invert INV-22.4-29 sub-(b) + INV-22.4-31 Strategy 2 + 2b in place; remove INV-22.4-31 /mouse sanity (INV-34 owns it); add INV-22.4-34 (dispatch_slash_no_strip_prefix). `cargo insta` re-baseline gate (zero diffs predicted per RESEARCH Finding 3). Implements D-02, D-05 (tui_rata 4-arm portion), D-06, D-07, D-09, D-10, D-11, D-12, D-13, D-14, D-15.
- [x] 22.4.1-02-PLAN.md — Wave 3: bulk Session + Configuration arm expansion — add 26 new `"<name>" => CommandResult::Output(...)` arms in `invoke_handler` (13 Session: history/save/retry/undo/title/compress/rollback/stop/background/btw/queue/status/resume — 13 Configuration: config/provider/prompt/personality/statusbar/verbose/yolo/reasoning/skin/voice/model/fast/debug). GatewayOnly names (approve/deny/sethome/start) excluded per RESEARCH Pitfall 6. Every arm carries the `Phase 22.4.1 stub:` marker per D-08; total marker count ≥ 30 (4 from Plan 01 + 26 from Plan 02). Add INV-22.4-33 (invoke_handler_arms) with per-name loop assertion + stub-marker count threshold. Implements D-05 (bulk portion), D-08, D-09, D-14.

**Wave structure:**
- Wave 1: 22.4.1-00 (core registry + INV-32 — autonomous; no file overlap with other plans)
- Wave 2: 22.4.1-01 (tui_rata pure-router refactor + 4 arms + render_help_router + INV-29/-31 inversion + INV-34 — depends on 22.4.1-00 because the router must resolve the 4 new names as ResolveResult::Exact before the fast-paths can be retired without behavior change; autonomous)
- Wave 3: 22.4.1-02 (26 bulk arms + INV-33 — depends on 22.4.1-01 because the new arms are appended to the same `invoke_handler` match table; autonomous)

**Test ledger:** Plan 00 leaves `cargo test -p ironhermes-cli --test invariants_22_4` at 32 tests; Plan 01 leaves it at 33; Plan 02 leaves it at 34. Final set is `{00..22, 24..34}` (INV-22.4-23 still deleted from Plan 22.4-16 precedent).

**Phase directory:** `.planning/phases/22.4.1-tui-rata-handler-re-port-route-dispatch-slash-through-comman/`

### Phase 22.3: REPL UX hardening (visual stability + reset + unified history) (INSERTED)

**Goal:** Close six concrete TTY-UX defects (D-1 ticker/output clobber, D-2 typo suggestions, D-3 alias→transcript race, D-4 banner bleed, D-6 `/clear` visual reset, D-7 unified persistent history) captured verbatim in 22.3-UAT-EVIDENCE.md, and re-pass the UAT scenario on a live TTY. UI-SPEC-locked contract (22.3-UI-SPEC.md): PaintCoordinator discipline, slash output block format, `/clear` (TTY visual reset, no history mutation), unified history at `$HERMES_HOME/repl_history` with rustyline 15 API (set_history_ignore_dups not HistoryDuplicates::Prev), TranscriptWriter touch-on-register, hand-rolled Levenshtein typo suggester (no new crate per Phase 21 D-18), and six static-grep regression invariants INV-22.3-01..06. Fix-up phase — no REQ-IDs map; UI-SPEC + UAT re-run serve as the requirements set.
**Requirements:** (none — UI-SPEC.md + 22.3-UAT-EVIDENCE.md serve as the requirements set; CONTEXT D-01..D-15 are locked decisions)
**Depends on:** Phase 22, Phase 22.1 (DECSTBM reserved-row formula), Phase 21.7 (readline barrier + transcript writer)
**Plans:** 12/12 plans complete

Plans:
- [x] 22.3-01-PLAN.md — Wave 1: Levenshtein typo suggester pure function (`commands::typo::suggest_typo`) with 10 unit tests; module declaration in `commands/mod.rs`. No new crate dep.
- [x] 22.3-02-PLAN.md — Wave 1: `TranscriptWriter::touch()` (sync std::fs OpenOptions create+append) called from `subagent_runner.rs` BEFORE `reg.write().await.register(info)` (corrected ordering — RESEARCH inverted CONTEXT D-07); integration test asserting file exists immediately after touch.
- [x] 22.3-03-PLAN.md — Wave 1: rustyline 15 history activation in `repl_input.rs` (corrected API: `set_history_ignore_dups(true)`, NotFound on first run silently ignored, `set_max_history_size(1000)`, save on Shutdown); `run_chat` passes `Some($HERMES_HOME/repl_history)` to `ReplInputChannel::spawn` (run_chat-only per CONTEXT D-15).
- [x] 22.3-04-PLAN.md — Wave 1: `CommandResult::ResetTerminal` unit variant added to BOTH core and TUI enums + mapper arm in `tui/commands.rs:map_core_to_tui`; `cmd_clear` switched from `ClearSession` to `ResetTerminal` (cmd_new unchanged — preserves /new truncate semantics).
- [x] 22.3-05-PLAN.md — Wave 2: `run_chat` integration — new `tui::render::reset_terminal_visual(reserved)` helper (DECSTBM-aware scrollback wipe + prompt re-anchor); ResetTerminal arms in prompt-time + mid-turn matches (RESEARCH §Pitfall 5 exhaustive-match closure); slash-side `repl_input.add_history(&input)` at prompt-time site (mid-turn skipped per HIST-8/INV-22.3-06); `suggest_typo` plugged into `cmd_agents` `Some(other)` arm (locked candidates `["list","kill","logs"]`) and `dispatch_command` `ResolveResult::NotFound` arm (router-derived candidates with `Type /help` fallback). Closes the workspace-build-failure gap that Plan 04 deliberately opened.
- [x] 22.3-06-PLAN.md — Wave 2: Six static-grep invariants `crates/ironhermes-cli/tests/invariants_22_3.rs` (INV-22.3-01..06): ResetTerminal arm exists, banner called once before TUI init, cmd_clear returns ResetTerminal + cmd_new unchanged, slash add_history after starts_with('/'), correct rustyline 15 API used + wrong API names absent, total add_history count == 2 (mid-turn has none). Pairs with Plan 22.3-02's runtime transcript-touch test for INV-22.3-05's behavioral half. No new dev-deps (Phase 21 D-18, CONTEXT D-03).
- [x] 22.3-07-PLAN.md — Wave 1 (gap-closure for WR-01): Migrate `TranscriptWriter::touch()` from sync `std::fs::OpenOptions` to async `tokio::fs::OpenOptions` so the call does not block a tokio runtime worker thread on slow/remote filesystems. Awaits the new method at the single call site in `subagent_runner.rs::run_child`. Migrates 2 `#[test]` integration tests in `tests/transcript_touch.rs` to `#[tokio::test]`.
- [x] 22.3-08-PLAN.md — Wave 1 (gap-closure for WR-02): Prepend `let _ = std::io::stdout().flush();` to the body of `reset_terminal_visual` in `tui/render.rs` so any buffered streaming token bytes drain to the terminal BEFORE the scrollback-erase escape fires on stderr. One-line addition. Smoke test preserved.
- [x] 22.3-09-PLAN.md — Wave 1 (gap-closure for WR-03): Extend `ReplInputChannel::shutdown(self)` (signature becomes `shutdown(mut self)`) to `self.worker.take()` and `handle.join()` after sending `Command::Shutdown` so `rl.save_history(path)` completes BEFORE shutdown returns — closes the history-loss window in the emergency-exit path.
- [x] 22.3-10-PLAN.md — Wave 1 (gap-closure for WR-04): Replace the stale `// /clear: wipe messages but keep session alive` comment above the `CoreCommandResult::ClearSession` arm in `crates/ironhermes-gateway/src/handler.rs` with a multi-line block accurately documenting that `/clear` now returns `ResetTerminal` (Phase 22.3 D-06) and that the arm is preserved for forward compatibility. Comment-only — runtime behavior unchanged.
- [x] 22.3-11-PLAN.md — Wave 2 (gap-closure for GAP-22.3-01 — BLOCKING): New `pub fn tui::render::write_into_scroll_region(bytes: &[u8], reserved: u16)` helper that wraps every write in DECSC (`7`) → absolute CUP to scroll-region last row → write+flush → DECRC (`8`). Routes `run_chat`'s `run_agent_turn` streaming-token callback (main.rs:~1682) and the post-turn `Hermes:` label (main.rs:~1034) through the helper. Eliminates the streaming-clobber UAT defect. Non-TTY fallback writes plain stdout. CONTEXT D-15: `run_chat`-only — `run_single`'s streaming callback at main.rs:528 intentionally untouched.
- [x] 22.3-12-PLAN.md — Wave 3 (gap-closure lockdown): Three new static-grep regression gates INV-22.3-07/08/09 in sibling test file `crates/ironhermes-cli/tests/invariants_22_3_streaming.rs` locking the Plan 22.3-11 streaming-discipline fix: helper exists + is re-exported + imported by main.rs (07); `run_agent_turn` body uses helper, raw `print!("{}", delta)` is gone from inside `run_agent_turn` but still present in `run_single` per D-15 (08); DECSTBM/DECSC/DECRC bytes do NOT appear inline in main.rs — encapsulation invariant (09). Original 6-test file untouched.

**Wave structure:**
- Wave 1 (parallel, autonomous): 22.3-01..04 (original) + 22.3-07/08/09/10 (gap-closure WR-01..04). Zero file overlap among the four gap-closure Wave 1 plans (transcript+subagent_runner, render.rs, repl_input.rs, gateway/handler.rs).
- Wave 2 (sequential, autonomous, depends on Wave 1): 22.3-05 (`run_chat` integration), then 22.3-06 (original INV regression tests). Then 22.3-11 (streaming-discipline GAP-22.3-01 closure — depends on 22.3-08 only because both touch render.rs).
- Wave 3 (autonomous, depends on 22.3-11): 22.3-12 (new INV gates locking the streaming-discipline fix).

**Live-TTY HUMAN-UAT:** Per CONTEXT D-04, after ALL Phase 22.3 plans land (including 22.3-11 / 22.3-12), the operator re-runs the exact 3-concurrent-subagent LoRA-research scenario from `22.3-UAT-EVIDENCE.md` and records pass/fail for D-1..D-7 (minus D-5) and the four GAP-22.3-01 `required_behavior` bullets in `22.3-HUMAN-UAT.md`. Operator task — NOT in any plan's scope.

**Phase directory:** `.planning/phases/22.3-repl-ux-hardening-visual-stability-reset-unified-history/`

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

### Phase 22.2: ACP Adapter — DEFERRED to v2.1

**Status:** Deferred (2026-04-27, per `.planning/v2.0-MILESTONE-AUDIT.md`)
**Goal:** [To be planned in v2.1]
**Requirements:** CLI-03, CLI-04, CLI-05, CLI-06, CLI-07, CLI-08 (now in REQUIREMENTS.md "Future Requirements → Deferred from v2.0")
**Depends on:** Phase 22
**Plans:** 0 plans (deferred — not broken down)

Phase 22.2 was never broken into plans during v2.0. The ACP adapter is a substantive new subsystem (JSON-RPC stdio server, SessionManager, event/permission/tool bridges, cwd-bound sessions) and nothing else in v2.0 depends on it. Per milestone audit, the four core v2.0 user flows (chat REPL, Telegram gateway, skills install, subagent delegation) all complete without ACP. Re-open as a fresh phase in v2.1 with `/gsd-discuss-phase` then `/gsd-plan-phase`.

Artifacts preserved (do not delete): `.planning/phases/22.2-acp-adapter/22.2-CONTEXT.md` and `22.2-DISCUSSION-LOG.md`.

Plans:
- [ ] DEFERRED — moved to v2.1 (re-plan when v2.1 milestone opens)

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

### Phase 21.8: skill remote download and install from skills.sh (INSERTED)

**Goal:** Port the install/update path from the open-source `skills` CLI (TypeScript) into the Rust `ironhermes-hub` crate, targeting `https://skills.sh` as the remote source. Replace the broken `SkillsShSource` registry adapter with a new blob-API adapter (three-hop pipeline: GitHub Trees API -> raw.githubusercontent -> skills.sh /api/download/<owner>/<repo>/<slug>, path-based URL). Introduce `skills-lock.json` v1 (merge-clean, alphabetically sorted, timestamp-free hashed region) replacing Phase 19.1 per-skill manifest files. Land all mandatory security primitives (terminal escape stripping D-16, YAML-only frontmatter D-17, path traversal guards D-18, pre-install audit D-19, temp-dir containment D-20). CLI surface: `hermes skills install|list|remove|update` with `remove` replacing `uninstall` (alias retained). Closes the Skills Hub clause of the v2.0 Active `Skill framework` requirement for the skills.sh surface.

**Requirements:** SKILL-08, MEM-06
**Depends on:** Phase 21
**Plans:** 6/6 plans complete

Plans:
- [x] 21.8-01-PLAN.md — Wave 0 test infra: create `sanitize.rs` with 9 pure-function security primitives (D-16/D-17/D-18/D-20), extend `HubErrorKind` with ShaMismatch/ScanHit/PathTraversal/Audit (D-24), add `to_skill_slug` golden-vector integration test (20+ cases, reference TS byte-for-byte match).
- [x] 21.8-02-PLAN.md — Wave 1 data + network: create `blob.rs` (SkillsShBlobSource, 3-hop fetchers, with_one_retry wrapper, D-06 corrected path-based URL, D-08 10s timeout, D-22 User-Agent) and `lock.rs` (SkillLock/SkillLockEntry camelCase schema, compute_folder_hash with NO separator per D-13 corrected, D-12 alphabetical sort + atomic save, paths::skills_lock_path).
- [x] 21.8-03-PLAN.md — Wave 2 pipeline rework: create `audit.rs` (fetch_audit soft-fail, D-19 3s timeout), add `migrate_from_hub_manifest` to lock.rs (D-15 idempotent 19.1->21.8), rework installer.rs to write SkillLock (not HubManifest), insert audit between fetch+quarantine, gate remove_dir_all with assert_temp_contained, verify post-rename computed_hash vs server snapshot_hash (ShaMismatch path), deprecate HubManifest::save.
- [x] 21.8-04-PLAN.md — Wave 3 CLI rework: delete skills_sh.rs + skills_sh_adapter_test.rs (D-01), swap skills_cmd.rs:136 and skills_tool.rs:382 call sites to SkillsShBlobSource, rename SkillsAction::Uninstall -> Remove with `#[command(alias = "uninstall")]` (D-04), add `--skip-audit` flag (D-19), emit D-21 5-line progress + D-23 restart message, route every server-originated stderr/stdout through strip_terminal_escapes (D-16 at print boundary), wire migrate_from_hub_manifest at CLI startup (D-15).
- [x] 21.8-05-PLAN.md — Wave 4 end-to-end integration: create wiremock integration test suite covering happy path, exactly-once retry on 5xx, no retry on 404 / PathTraversal, path-traversal rejection before disk write, User-Agent openclaw ride capture, audit soft-fail (timeout/5xx/non-json), --skip-audit zero-network bypass, idempotent migration (byte-identical on re-run), cmd_install -> cmd_list -> cmd_remove round-trip; full `cargo test --workspace` green gate.
- [x] 21.8-06-PLAN.md — Wave 5 gap closure: realign installer post-install hash compare with D-14 opaque contract — make server-vs-client snapshotHash equality ADVISORY (tracing::warn, not ShaMismatch) in install()/update(), preserve D-13 compute_folder_hash as the client-authoritative drift sentinel, add unit + wiremock integration tests locking the divergence path; unblocks UAT Tests 3 + 4 which 100% failed on live skills.sh due to server/client hash algorithm divergence (G-01).

**Wave structure:**
- Wave 1: 21.8-01 (sanitize.rs + HubErrorKind + slug golden vectors — autonomous)
- Wave 2: 21.8-02 (blob.rs + lock.rs + paths — depends on 01, autonomous)
- Wave 3: 21.8-03 (audit.rs + installer.rs rework + migration + manifest deprecation — depends on 01, 02, autonomous)
- Wave 4: 21.8-04 (delete skills_sh.rs, CLI rework, call-site swaps, D-21/D-23 UX, strip at print boundary — depends on 01, 02, 03, autonomous)
- Wave 5: 21.8-05 (wiremock e2e + audit/migration/CLI integration tests — depends on 01, 02, 03, 04, autonomous)
- Wave 5: 21.8-06 (gap closure: advisory snapshotHash compare — realigns with D-14; no structural deps on prior waves, autonomous)

**Phase directory:** `.planning/phases/21.8-skill-remote-download-and-install-from-skills-sh/`

### Phase 21.7: Multi-agent and autonomous agents and sandbox status (INSERTED)

**Goal:** Close four v2.0 hermes-agent parity gaps scoped to "minimum viable parity": (a) surface the existing `delegate_task` subagent system with `/agents`, status-line pill, persistent JSONL transcripts, cascade cancellation, wall-clock timeout, concurrency surface, and parent/child iteration-budget inheritance; (b) add `--yolo` non-interactive mode that bypasses dangerous-command approvals while the iteration budget, ctrl-c cascade, and fatal-error halt remain unskippable; (c) add `hermes status [--all] [--deep] [--json]` component diagnostics; (d) add an in-memory session-scoped `ProcessRegistry` for `terminal(background=true)` / `execute_code` with spawn/poll/wait/kill, 200KB rolling output buffer, `/stop` slash, watch patterns with rate-limited notifications, and cleanup on session end. Explicit exclusions (deferred to own phases): worktree-parallel mode, toolsets grouping, terminal backends, full gateway session mirror, plugin discovery sources, full approval queue. 29 locked CONTEXT decisions (D-01..D-29), 12 eval dimensions (E-01..E-12), 18 scenario fixtures (S-01..S-18), 10 online guardrails (G-01..G-10) — 5 of which remain unskippable under `--yolo` per the Replit-July-2025 anti-pattern anchor.

**Requirements:** PROV-09, PROV-10

**Depends on:** Phase 21, Phase 21.1, Phase 21.2, Phase 21.4, Phase 21.6

**Plans:** 11/11 plans complete

Plans:
- [x] 21.7-00-wave0-test-infra-PLAN.md — Wave 0 test infra + dev-deps (insta/assert_cmd/tracing-test/sysinfo/nix) + `ensure_home_dirs()` extension for `subagent-transcripts/` + `BudgetHandle` type shell + three static-grep regression gates (E-05/E-08/E-09)
- [x] 21.7-01-budget-handle-pressure-tiers-PLAN.md — `BudgetHandle` impl (consume/pressure/advisory_text) with SeqCst atomics + 10k-loop concurrent-consume stress test (E-05/S-13) — PROV-09/PROV-10 type foundation
- [x] 21.7-02-process-registry-module-PLAN.md — `ProcessRegistry` + `ProcessSession` module (`ironhermes-exec/src/process_registry.rs`): constants verbatim from hermes-agent, spawn/poll/wait/kill/logs, LRU prune at 64, TTL 30min, watch-pattern rate limiter with sustained-overload auto-disable (D-23..D-29, E-03/E-04)
- [x] 21.7-03-subagent-registry-transcript-PLAN.md — `SubagentRegistry` (D-03/D-04/D-09) + `TranscriptWriter` fire-and-forget JSONL with cancellation marker (D-05/D-07, E-08)
- [x] 21.7-04-status-cmd-skeleton-deepprobe-PLAN.md — `status_cmd` module skeleton + v1 JSON schema locked via insta snapshot + `DeepProbe` trait seam with `LiveDeepProbe` stubs + `MockDeepProbe` for fault injection (D-18..D-22, E-06/E-07)
- [x] 21.7-05-budget-handle-three-site-wiring-PLAN.md — Wire BudgetHandle through `AgentSubagentRunner` + `AgentLoop::run_agent_turn` decrement-at-top + pressure-advisory injection on tier crossings + three `main.rs` registration sites + gateway (PROV-09/PROV-10 integration, E-12)
- [x] 21.7-06-process-registry-wiring-PLAN.md — Wire ProcessRegistry into `terminal(background=true)` + `execute_code(background=true)` + three `on_session_end` call sites (CLI + gateway) + stdout-drain tasks that feed `ingest_output` (D-24/D-27/D-29, E-03 end-to-end with `sysinfo`+`waitpid` no-zombie assertion)
- [x] 21.7-07-subagent-registry-pill-transcript-wiring-PLAN.md — Extend `CommandContext` with 4 new handles (process/subagent/budget/semaphore via `ironhermes-core` trait objects) + `agents: N/M` status-line pill with hide-at-zero (Pitfall 8) + thread SubagentRegistry + TranscriptWriter through AgentSubagentRunner lifecycle (D-03/D-04/D-05/D-07/D-09, E-11)
- [x] 21.7-08-yolo-and-slash-handlers-PLAN.md — `--yolo` CLI flag + `autonomous.yolo` config key (CLI wins) + one-shot stderr banner + approval skip-under-yolo gate + non-TTY `IsTerminal` gate + fill `/agents list|kill|logs` and `/stop` handlers + 2s debounced semaphore-wait warn (D-09/D-11/D-12/D-13/D-14/D-26, E-02/E-10, G-08/G-10, Pitfall 10)
- [x] 21.7-09-hermes-status-cli-wiring-PLAN.md — Wire `hermes status` into `Commands::Status` + fill `StatusReport::collect` for all four D-18 sections + implement `LiveDeepProbe` real probes (provider HEAD + FTS5 integrity + MCP honest-unreachable fallback) + **D-08 fix**: `delegate_task.rs:265-283 / :547-579` timeout arms now explicitly call `child_cancel_token.cancel()` BEFORE bail (AI-SPEC Pitfall 5) + `timeout_seconds` schema field for per-call override
- [x] 21.7-10-eval-scenarios-and-ci-gates-PLAN.md — S-01..S-18 scenario tests (cascade-cancel, yolo-guardrails, process-registry) + M-01 cascade-cancel p95 < 500ms bench + `scripts/ci-gates.sh` with four static-grep gates (E-05/E-08/E-09/D-12) + `cargo insta test --unreferenced=reject --workspace` + optional CI workflow edit

**Wave structure:**
- Wave 0: 21.7-00 (test infra + dev-deps + home-dirs + static-grep gates + BudgetHandle shell — autonomous; blocks all downstream)
- Wave 1: 21.7-01, 21.7-02, 21.7-03, 21.7-04 in parallel (new types: BudgetHandle impl, ProcessRegistry, SubagentRegistry+transcript, status_cmd skeleton — all autonomous, all depend on Wave 0)
- Wave 2: 21.7-05, 21.7-06, 21.7-07 in parallel (integration: BudgetHandle three-site, ProcessRegistry wiring + on_session_end, SubagentRegistry+pill+transcript wiring — all autonomous, depend on Wave 1 respective foundations)
- Wave 3: 21.7-08, 21.7-09 in parallel (CLI surface: --yolo + /agents + /stop + banner + non-TTY gate; hermes status + --deep + D-08 timeout fix — both autonomous, depend on Wave 2)
- Wave 4: 21.7-10 (eval scenarios + p95 bench + CI gates — autonomous, depends on Waves 0-3)

**AI integration:** Framework is IronHermes itself (Rust; no new framework per AI-SPEC §2). 5 critical failure modes, 12 eval dimensions, 18 scenario fixtures, 10 guardrails (6 online + 4 offline), `tracing`-based observability with 4 new structured event targets. Replit-July-2025 incident cited as anti-pattern anchor for `--yolo` safety contract.

**Phase directory:** `.planning/phases/21.7-multi-agent-and-autonomous-agents-and-sandbox-status/`

### Phase 21.6: Port deployment setup files from hermes-agent (INSERTED)

**Goal:** Port hermes-agent's deployment and setup infrastructure to IronHermes: .env.example env var template, cli-config.yaml.example config template, Dockerfile with multi-stage Rust build, docker/entrypoint.sh bootstrap script, install.sh curl-pipe installer, setup-ironhermes.sh post-clone dev setup, and first-run directory scaffolding in main.rs. D-01..D-24 from CONTEXT.md serve as requirements.
**Requirements:** D-01, D-02, D-03, D-04, D-05, D-06, D-07, D-08, D-09, D-10, D-11, D-12, D-13, D-14, D-15, D-16, D-17, D-18, D-19, D-20, D-21, D-22, D-23, D-24
**Depends on:** Phase 21
**Plans:** 3/3 plans complete

Plans:
- [x] 21.6-01-PLAN.md — Create .env.example (all provider/tool/gateway env vars, commented-out), cli-config.yaml.example (full Config struct mirror with inline docs), .dockerignore, docker/SOUL.md default identity seed, and ensure_home_dirs() first-run directory scaffolding in main.rs
- [x] 21.6-02-PLAN.md — Create Dockerfile (multi-stage: gosu + rust:latest builder + debian:bookworm-slim runtime with python3, non-root ironhermes user UID 10000, IRONHERMES_HOME=/opt/data volume) and docker/entrypoint.sh (privilege drop via gosu, UID/GID remapping, directory creation, template seeding)
- [x] 21.6-03-PLAN.md — Create install.sh (curl-pipe end-user installer: OS/arch detection, GitHub Releases binary download with cargo install fallback, directory scaffolding, template seeding, PATH patching) and setup-ironhermes.sh (post-clone dev setup: Rust check, cargo build --release, symlink, config scaffolding)

**Wave structure:**
- Wave 1: 21.6-01 (config templates + .dockerignore + SOUL.md + ensure_home_dirs — autonomous)
- Wave 2: 21.6-02 and 21.6-03 in parallel (Dockerfile + entrypoint, install/setup scripts — both depend on 01, both autonomous)

**Phase directory:** `.planning/phases/21.6-port-deployment-setup-files-from-hermes-agent/`

### Phase 21.5: Memory Provider Plugin (INSERTED)

**Goal:** Make memory providers deliver on the "plugin" promise: factory config loading from $HERMES_HOME/<name>.json, unified memory_recall tool (FTS5 for SQLite, graph traversal for Grafeo, analytical ILIKE for DuckDB), provider-specific hook implementations (sync_turn, on_pre_compress, system_prompt_block, queue_prefetch), and agent_loop wiring to expose memory_recall to the LLM. D-01..D-13 from CONTEXT.md serve as requirements.
**Requirements:** D-01, D-02, D-03, D-04, D-05, D-06, D-07, D-08, D-09, D-10, D-11, D-12, D-13
**Depends on:** Phase 21.4
**Plans:** 4/4 plans complete

Plans:
- [x] 21.5-01-PLAN.md — Factory config loading: load_provider_config helper reads $HERMES_HOME/<name>.json (D-01/D-02), replace Value::Null stubs in both build_memory_provider and build_tokio_provider. Refactor SqliteMemoryProvider.conn to Arc<Mutex<Connection>> for tokio::spawn compatibility.
- [x] 21.5-02-PLAN.md — SQLite provider: memory_recall via FTS5 MATCH with bm25 ranking and snippet generation (D-03/D-05/D-11), handle_tool_call dispatch, sync_turn fire-and-forget FTS5 rebuild (D-07), on_pre_compress indexes compressed messages (D-08), system_prompt_block surfaces recent entries (D-10), queue_prefetch FTS5 cache warming (D-09).
- [x] 21.5-03-PLAN.md — Grafeo provider: memory_recall via content substring match with relevance scoring (D-12), entity extraction heuristic helper, system_prompt_block with knowledge graph summary (D-10). DuckDB provider: memory_recall via ILIKE bridge command (D-13), new fire-and-forget DuckDbCommand variants (SyncTurn/OnPreCompress/QueuePrefetch), system_prompt_block with analytical summary (D-10).
- [x] 21.5-04-PLAN.md — Agent loop wiring: inject memory provider tool schemas into LLM tool list via memory_manager.get_tool_schemas(), add memory_provider_tool_names HashSet field, intercept memory_recall calls before registry dispatch and route to MemoryManager.handle_tool_call (D-03/D-04).

**Wave structure:**
- Wave 1: 21.5-01 (factory config loading + SQLite Arc refactor — autonomous)
- Wave 2: 21.5-02 and 21.5-03 in parallel (SQLite hooks + Grafeo/DuckDB hooks — both depend on 01, both autonomous)
- Wave 3: 21.5-04 (agent loop wiring — depends on 02 and 03, autonomous)

**Phase directory:** `.planning/phases/21.5-memory-provider-plugin/`

### Phase 21.4: Persistent Memory gap analysis verification (INSERTED)

**Goal:** Systematic gap analysis comparing IronHermes' persistent memory implementation (Phases 11, 17, 20) against hermes-agent's reference documentation and provider lifecycle contract. Produce GAP-ANALYSIS.md audit report, then close all gaps: wire memory_manager into AgentLoop and context engine across CLI/gateway (queue_prefetch, on_pre_compress), add memory_enabled/user_profile_enabled config toggles, add `hermes memory status` and `hermes memory off` CLI subcommands, wire on_session_end in clean exit paths. Includes MEM-06 verification (pulled from Phase 15 scope -- confirmed already correct).
**Requirements:** D-01, D-02, D-03, D-04, D-05, D-06, D-07, D-08, D-09, D-10, D-11, D-12
**Depends on:** Phase 21
**Plans:** 3/3 plans complete

Plans:
- [x] 21.4-01-PLAN.md — Produce structured GAP-ANALYSIS.md audit report: feature-by-feature comparison against REFERENCE-hermes-agent-memory.md, provider lifecycle hook wiring matrix (11 hooks), MEM-06 frozen snapshot verification, 6 gaps catalogued with severity ratings
- [x] 21.4-02-PLAN.md — Close GAP-1/2/3/4: add memory_enabled and user_profile_enabled config toggles to MemoryConfig, update build_memory_manager to return Option (None when disabled), wire memory_manager into AgentLoop (run_agent_turn + gateway handler) and context engine (build_context_engine + attach_context_engine), static-grep regression tests
- [x] 21.4-03-PLAN.md — Close GAP-5/6: add hermes memory status (provider info, store sizes, mirror status) and hermes memory off (reset to file provider) CLI subcommands in memory_cmd.rs, wire on_session_end in run_single and run_chat clean exit paths

**Wave structure:**
- Wave 1: 21.4-01 (GAP-ANALYSIS.md audit report -- autonomous)
- Wave 2: 21.4-02 and 21.4-03 in parallel (config toggles + lifecycle wiring, CLI subcommands + on_session_end -- both autonomous)

**Phase directory:** `.planning/phases/21.4-persistent-memory-gap-analysis-verification/`

### Phase 21.3: Model metadata & models.dev — context lengths, token estimation (INSERTED)

**Goal:** Replace the hardcoded `DEFAULT_CONTEXT_LENGTH = 128_000` with a model-aware metadata system (static lookup table + disk cache from models.dev/OpenRouter APIs), replace the crude `text.len() / 4 + 1` token estimation heuristic with proper BPE tokenization via tiktoken-rs, wire accurate model metadata through all consumers (AgentLoop, ContextCompressor, PressureTracker, StatusLine), and add `hermes models list/fetch/info` CLI subcommand plus `/models` slash command. D-01..D-15 from CONTEXT.md serve as requirements.
**Requirements:** D-01, D-02, D-03, D-04, D-05, D-06, D-07, D-08, D-09, D-10, D-11, D-12, D-13, D-14, D-15
**Depends on:** Phase 21
**Plans:** 5/5 plans complete

Plans:
- [x] 21.3-01-PLAN.md — Create ModelMetadata/ModelCapabilities/ModelRegistry structs with static lookup table (30+ models across 7 families), canonical ID + alias map (versioned/prefixed/legacy name resolution), TokenEstimator wrapping tiktoken-rs singletons (cl100k_base + o200k_base), global estimator with OnceLock, warm function. All in ironhermes-core with comprehensive TDD unit tests.
- [x] 21.3-02-PLAN.md — Wire metadata through resolution chain: add model_metadata to ResolvedEndpoint, populate from ModelRegistry in ProviderResolver::build(), replace text.len()/4 heuristic with tiktoken in context_compressor, parameterize attach_context_engine with context_length, update all four hardcoded 128_000 sites in main.rs and gateway handler.
- [x] 21.3-03-PLAN.md — Implement disk cache (ModelsCache with load/save to ~/.ironhermes/models-cache.json) and API fetch layer (models.dev primary + OpenRouter fallback per D-03), parse functions for both API response formats, normalize_model_id, fetch_all with fallback chain and FetchResult reporting.
- [x] 21.3-04-PLAN.md — Add hermes models list/fetch/info CLI subcommands (models_cmd.rs following cron.rs pattern, UI-SPEC terminal output contracts) and /models refresh|info slash commands (plain text CommandResult::Output, no ANSI codes). Wire into Commands enum and CommandRouter registry.
- [x] 21.3-05-PLAN.md — Gap closure: Wire ModelsCache::load() + merge_cache() into ProviderResolver::build() so disk cache is auto-loaded at startup for all runtime entry points (D-02, D-06 completion). Regression tests.

**Wave structure:**
- Wave 1: 21.3-01 (ModelMetadata + TokenEstimator + static table — autonomous)
- Wave 2: 21.3-02 and 21.3-03 in parallel (wiring + cache/fetch — both depend on 01, both autonomous)
- Wave 3: 21.3-04 (CLI subcommand + slash command — depends on 02 and 03, autonomous)
- Wave 4: 21.3-05 (gap closure: disk cache auto-load — autonomous)

**Phase directory:** `.planning/phases/21.3-model-metadata-models-dev-context-lengths-token-estimation/`

### Phase 21.2: MCP client tool and fold in slash commands related to MCP client use (INSERTED)

**Goal:** Port hermes-agent's MCP client infrastructure to IronHermes: new `ironhermes-mcp` crate using the official `rmcp` SDK for stdio and HTTP/StreamableHTTP transports, per-server tokio tasks with exponential backoff reconnection, tool discovery and registration into a dynamically-mutable `Arc<RwLock<ToolRegistry>>`, sampling support, credential stripping, safe env filtering, `/reload-mcp` slash command, and `hermes mcp add/remove/list/test/configure` CLI subcommands. D-01..D-21 from CONTEXT.md serve as requirements.
**Requirements:** D-01, D-02, D-03, D-04, D-05, D-06, D-07, D-08, D-09, D-10, D-11, D-12, D-13, D-14, D-15, D-16, D-17, D-18, D-19, D-20, D-21, GAP-5, GAP-6, GAP-7, GAP-8
**Depends on:** Phase 21
**Plans:** 11/11 plans complete

Plans:
- [x] 21.2-01-PLAN.md — Create ironhermes-mcp crate scaffold with rmcp dependency, McpServerConfig (hermes-agent-compatible YAML schema), env var interpolation, build_safe_env allowlist, sanitize_error credential stripping, and mcp_servers field on Config struct
- [x] 21.2-02-PLAN.md — Add register_dynamic/unregister_by_prefix to ToolRegistry and migrate all Arc<ToolRegistry> callsites to Arc<RwLock<ToolRegistry>> across 6 files (rpc_registry stays Arc<ToolRegistry> for safe subset isolation)
- [x] 21.2-03-PLAN.md — Implement McpManager orchestrating per-server tokio tasks, McpTool (Tool trait impl with channel-based dispatch), stdio/HTTP transport helpers via rmcp SDK, sampling handler with rate limiting, exponential backoff reconnection (5 retries, max 60s), tool naming (server__tool), description prefixing ([MCP: server_name]), enabled_tools filtering, and reload capability
- [x] 21.2-04-PLAN.md — Add McpReloader trait to CommandContext (circular-dep resolution), wire /reload-mcp and /reload handlers replacing todo stubs, integrate McpManager into run_chat/run_single/run_gateway with background discovery, handle McpReload CommandResult in REPL loop
- [x] 21.2-05-PLAN.md — Implement hermes mcp add/remove/list/test/configure CLI subcommands in mcp_config.rs with interactive wizard, config.yaml persistence, test connection via rmcp, and UI-SPEC styled output (colored crate matching cron.rs patterns)
- [x] 21.2-06-PLAN.md — GAP-1/2/3 close: attempt_connect_and_list_with_timeout, RetrySaveAbort 3-way prompt, literal-copy regression tests
- [x] 21.2-07-PLAN.md — GAP-4 close: sanitize_server_name single source, broadened sanitizer (@/), symmetric register/unregister
- [x] 21.2-08-PLAN.md — GAP-5 close: flush banner to stdout before prompt (pending sequential execution)
- [x] 21.2-09-PLAN.md — GAP-6 close: context-aware tracing init (interactive REPL → error filter) + stdio child stderr piped (Stdio::piped) + 2 regression tests
- [x] 21.2-10-PLAN.md — GAP-7 close: pending
- [x] 21.2-11-PLAN.md — GAP-8 close: ironhermes gateway Ctrl+C hang (pending)

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

---

## v2.1: Carry-Overs + Learning Loop

> **Milestone goal:** Close all 29 v2.0 deferred requirements across 7 categories **AND** land the Learning Loop foundation (5 new reqs, 2 new phases). The Learning Loop — periodic memory nudge + autonomous skill creation — is the canonical hermes-agent differentiator that makes the agent self-improving rather than just feature-complete.
> **Phases 23-31:** carry-over work (CFG, TOOL, PROV, PRMT, SKILL trust tiers, gateway formal verification, ACP adapter)
> **Phases 32-33:** Learning Loop foundation (LEARN-01..05) — agent-curated memory + autonomous skill creation
> **Total:** 11 phases, 34 reqs across 8 categories
> Phase numbering continues from v2.0 last phase (22.4.2.3). New phases start at 23.

**Architectural principles** (carried through every v2.1 phase, sourced from canonical hermes-agent design):
1. The Learning Loop is the unifying philosophy — Skills + Memory + Session Search are outputs of one continuous process
2. Cache-awareness is load-bearing — three cache breakers (model switch, memory file change, context file change) must be enforced (Phase 27) and surfaced in config UX (Phases 23/25/26)
3. 3,575 char total memory limit (already aligned: MEM-01 + MEM-02 = 3,575)
4. Patch-over-rewrite for skill self-improvement (Phase 33 default)
5. Progressive disclosure for token economy (Phase 28 + 33)
6. Sessions tied to ID, not platform (Phase 29 + 30/31)
7. Gateway as same-loop participant, not bolt-on (Phase 29)

### Phase 23: Configuration CLI and Setup Wizard

**Goal:** Users can configure IronHermes interactively on first run and manage config values from the command line.
**Depends on:** Phase 21 (config infrastructure), Phase 20 (memory setup wizard pattern)
**Requirements:** CFG-01, CFG-02, CFG-03
**Success Criteria** (what must be TRUE):
  1. Running `hermes` for the first time launches an interactive setup wizard that asks for provider selection, API key, model, and writes a valid `config.yaml`
  2. `hermes config set <key> <value>` updates a config.yaml key and `hermes config get <key>` reads it back
  3. `hermes config show` prints the active config with redacted secrets
  4. `hermes config migrate` scans installed skills for unconfigured settings and prompts the user to fill them in
**Plans:** 2/2 plans complete

Plans:
- [x] 23-01-PLAN.md — Schema extension + pure-function core (wizard, validate, dotted-path setter) + Wave 0 test scaffolding
- [x] 23-02-PLAN.md — CLI surfaces (`hermes setup` + `hermes config`) + rustyline I/O + first-run pre-flight middleware + manual UAT

**Phase directory:** `.planning/phases/23-configuration-cli-and-setup-wizard/`

### Phase 24: Profile Isolation

**Goal:** Each named profile gets its own isolated HERMES_HOME, config, memory stores, sessions database, and gateway PID file — operator can switch between profiles without cross-contamination.
**Depends on:** Phase 23 (config CLI must exist before profiles can reference configs)
**Requirements:** CFG-04
**Success Criteria** (what must be TRUE):
  1. `hermes --profile work chat` uses `~/.ironhermes/profiles/work/` as HERMES_HOME, separate from default
  2. Memory stores and session history for `work` profile are completely isolated from `personal` profile
  3. Gateway started under one profile does not interfere with gateway under another profile (separate PID files)
  4. Profile directory is scaffolded automatically on first use with the same `ensure_home_dirs()` structure as default
**Plans:** 3/7 plans executed

Plans:
- [ ] TBD (run /gsd-plan-phase 24 to break down)

**Phase directory:** `.planning/phases/24-profile-isolation/`

### Phase 25: Toolset Management

**Goal:** Tools are organized into named toolsets with runtime enable/disable, prerequisite check functions that silently exclude unavailable tools from the LLM schema, and a setup wizard hook that guides users through missing tool prerequisites.
**Depends on:** Phase 23 (setup wizard integration requires CFG-01 wizard infrastructure), Phase 21.1 (slash command registry for toolset commands)
**Requirements:** TOOL-01, TOOL-02, TOOL-03, TOOL-04, TOOL-05
**Success Criteria** (what must be TRUE):
  1. Each tool has an `is_available()` check; tools whose prerequisites (env vars, API keys) are absent are silently excluded from the schema sent to the LLM
  2. Tools are grouped into named toolsets (e.g., `web`, `code`, `memory`) and operator can list/enable/disable a toolset at runtime
  3. Adding a new tool requires only a registration call — no changes to dispatch logic
  4. Agent-intercepted tools (memory, session_search, delegate_task) are handled before registry dispatch without being visible to the LLM as duplicates
  5. `hermes setup` (or first-run wizard) detects tools with missing prerequisites and guides the user through configuring them
**Plans:** TBD

Plans:
- [ ] TBD (run /gsd-plan-phase 25 to break down)

**Phase directory:** `.planning/phases/25-toolset-management/`

### Phase 26: Provider Polish

**Goal:** API keys are scoped to their provider's base URL, auxiliary tasks can route to a separate cheaper model, and operators can define named custom providers in config.yaml for any OpenAI-compatible endpoint.
**Depends on:** Phase 21 (ProviderResolver infrastructure), Phase 23 (config CLI for setting provider values)
**Requirements:** PROV-04, PROV-06, PROV-08
**Success Criteria** (what must be TRUE):
  1. Configuring two providers with different base URLs and different API keys sends the correct key to each endpoint — no key leaks to the wrong URL
  2. Setting `auxiliary_model` in config.yaml routes compression, vision, and session-search tasks to that model instead of the main conversational model
  3. A named custom provider (e.g., `my-local-llm`) defined in config.yaml is selectable as `--provider my-local-llm` and resolves its base URL, API key, and model correctly
**Plans:** TBD

Plans:
- [ ] TBD (run /gsd-plan-phase 26 to break down)

**Phase directory:** `.planning/phases/26-provider-polish/`

### Phase 27: Prompt Caching

**Goal:** Anthropic Claude API calls automatically use `cache_control` breakpoints via the system_and_3 strategy, reducing cost and latency for repeated prefixes.
**Depends on:** Phase 26 (PROV-04 key scoping ensures Anthropic requests use the correct key before caching is wired), Phase 15 (10-layer prompt assembly)
**Requirements:** PRMT-08, PRMT-09
**Success Criteria** (what must be TRUE):
  1. When using an Anthropic Claude model, the system prompt and last 3 non-system messages carry `cache_control` breakpoints in the request payload
  2. Prompt caching is automatically enabled for Anthropic models and silently skipped for non-Anthropic providers — no config change needed
  3. The configurable TTL (5m or 1h) is respected in the `cache_control` type field
**Plans:** TBD

Plans:
- [ ] TBD (run /gsd-plan-phase 27 to break down)

**Phase directory:** `.planning/phases/27-prompt-caching/`

### Phase 28: Skills Trust Tiers

**Goal:** Installed skills carry a trust level (builtin / official / trusted / community) that drives security enforcement — community skills face stricter scanning gates than builtin skills.
**Depends on:** Phase 21.8 (skills lock + install pipeline that this tier system annotates), Phase 25 (toolset management — trust tier enforcement reuses the is_available() check pattern)
**Requirements:** SKILL-09
**Success Criteria** (what must be TRUE):
  1. Skills shipped inside the binary are classified as `builtin`; skills from the optional-skills/ directory as `official`; skills from known repo sources as `trusted`; all others as `community`
  2. Community skills that fail the security scan are hard-rejected at load time; builtin/official/trusted skills that fail emit a warning but still load
  3. `hermes skills list` shows each skill's trust tier alongside its name and status
**Plans:** TBD

Plans:
- [ ] TBD (run /gsd-plan-phase 28 to break down)

**Phase directory:** `.planning/phases/28-skills-trust-tiers/`

### Phase 29: Gateway Formal Verification

**Goal:** The existing `ironhermes-gateway` crate has formal test coverage for all architectural contracts: session key construction, two-level message guard, authorization, hook lifecycle events, delivery routing, token locks, and background maintenance — back-filling verification that implementation matches spec.
**Depends on:** Phase 21 (gateway architecture already implemented), Phase 21.1 (slash command dispatch verified separately)
**Requirements:** GW-01, GW-02, GW-03, GW-04, GW-06, GW-07, GW-09, GW-10
**Success Criteria** (what must be TRUE):
  1. `build_session_key()` produces the documented `agent:main:{platform}:{chat_type}:{chat_id}` format and tests cover all platform/chat_type combinations
  2. The two-level message guard is tested: base adapter queues messages when agent is active, and gateway runner bypasses /stop /approve /deny while blocking other commands
  3. Authorization allowlist tests confirm that messages from non-whitelisted chats are denied and DM pairing codes gate access correctly
  4. Hook lifecycle event tests confirm `gateway:startup`, `session:start/end`, `agent:start/step/end` fire at the correct points in the message processing pipeline
  5. Token lock tests confirm `acquire_scoped_lock` / `release_scoped_lock` prevent two gateway instances from sharing the same bot token
**Plans:** TBD

Plans:
- [ ] TBD (run /gsd-plan-phase 29 to break down)

**Phase directory:** `.planning/phases/29-gateway-formal-verification/`

### Phase 30: ACP Adapter Core

**Goal:** IronHermes exposes a JSON-RPC stdio server that VS Code, Zed, and JetBrains can connect to, with a SessionManager that creates, forks, and manages isolated agent sessions bound to the editor's working directory.
**Depends on:** Phase 22 (CLI infrastructure, AgentLoop wiring), Phase 22.2 (ACP context and discussion artifacts), Phase 24 (profile isolation — ACP sessions benefit from per-session isolation)
**Requirements:** CLI-03, CLI-04, CLI-08
**Success Criteria** (what must be TRUE):
  1. `hermes acp` starts a JSON-RPC stdio server; a client can send a `session.create` request and receive a session ID in response
  2. Each ACP session is bound to the editor's cwd at creation time; file and terminal tool calls resolve relative to that cwd
  3. `session.fork` creates a child session inheriting parent context; `session.list` enumerates active sessions; `session.remove` tears down a session cleanly
  4. Sessions survive across multiple JSON-RPC calls within the same stdio connection and are cleaned up when the connection closes
**Plans:** TBD

Plans:
- [ ] TBD (run /gsd-plan-phase 30 to break down)

**Phase directory:** `.planning/phases/30-acp-adapter-core/`

### Phase 31: ACP Adapter Bridges

**Goal:** The ACP server translates AgentLoop callbacks into editor-facing session_update events, maps dangerous-command approval requests to ACP permission flow, and renders Hermes tool outputs (file diffs, shell commands, text previews) in editor-native content formats.
**Depends on:** Phase 30 (ACP server and SessionManager must exist before bridges can be wired)
**Requirements:** CLI-05, CLI-06, CLI-07
**Success Criteria** (what must be TRUE):
  1. AgentLoop streaming events (tool_progress, thinking, step, stream_delta) appear as `session_update` JSON-RPC notifications in the editor within 100ms of the event firing
  2. When a tool requires dangerous-command approval, the editor receives a permission request and `allow_once` / `allow_always` / `reject` responses are correctly honored by the agent
  3. File-write tool calls produce a diff-format content block; shell-command tool calls produce a command-preview block; text-generation produces a plain text block — all in the ACP content schema
**Plans:** TBD

Plans:
- [ ] TBD (run /gsd-plan-phase 31 to break down)

**Phase directory:** `.planning/phases/31-acp-adapter-bridges/`

### Phase 32: Periodic Nudge & Memory Curation

**Goal:** Land the agent-curated memory side of the Learning Loop. At configurable intervals during a session, the agent receives an internal system-level prompt asking it to review recent activity and decide what is worth persisting to MEMORY.md/USER.md vs leaving in the SQLite session archive. Honors PRMT-06 (mid-session writes don't mutate the active prompt — they take effect at next session start).
**Depends on:** v2.0 memory framework (MEM-01..06 done); v2.0 PRMT-06 (frozen-at-session-start memory snapshot already shipped)
**Requirements:** LEARN-01, LEARN-02
**Success Criteria** (what must be TRUE):
  1. A periodic nudge fires at the configured interval (default 5 min) during an active chat session, injecting a system-level prompt without user input
  2. The agent can write to MEMORY.md/USER.md within the existing 3,575 char total cap during a nudge cycle; persisted entries appear in the next session's prompt without breaking the current session's prompt cache
  3. The agent demonstrably routes some items to prompt memory and others to session-search-only, exercising the "permanence threshold" judgment LEARN-02 specifies
  4. Nudge interval is configurable via `hermes config set learning.periodic_nudge_interval_seconds <N>` (Phase 23 setup wizard surfaces this option)
**Plans:** TBD (estimated 2 plans — nudge fire mechanism + memory persistence judgment prompt design)

Plans:
- [ ] TBD (run /gsd-plan-phase 32 to break down)

**Phase directory:** `.planning/phases/32-periodic-nudge-memory-curation/`

### Phase 33: Autonomous Skill Creation & Self-Improvement

**Goal:** Land the agent-curated skill side of the Learning Loop. At task completion, the agent evaluates whether the path is worth documenting via heuristic (5+ tool calls / error recovery / user correction / non-obvious workflow) and autonomously writes a SKILL.md following the agentskills.io standard. The new `skill_manage` tool exposes 6 actions (create/patch/edit/delete/write_file/remove_file) with `patch` preferred for token-efficient updates.
**Depends on:** Phase 25 (toolset registry — registers skill_manage as a new toolset entry); Phase 28 (SKILL-09 trust tiers — adds the `Self-created` tier that LEARN-04 assigns by default); v2.0 skill framework (Phase 19, done)
**Requirements:** LEARN-03, LEARN-04, LEARN-05
**Success Criteria** (what must be TRUE):
  1. After a task that hit a trigger heuristic completes, the agent emits a tool_call to `skill_manage(action="create", ...)` that produces a valid SKILL.md under `~/.hermes/skills/<category>/<slug>/SKILL.md` with the `Self-created` trust tier set
  2. Updates to existing skills are made via `skill_manage(action="patch", ...)` by default; the `patch` payload contains only the changed text, not the full skill content (token-efficient)
  3. All 6 actions (create, patch, edit, delete, write_file, remove_file) are exposed via the `skill_manage` tool with the same JSON schema shape as the existing memory tool actions; runtime tests confirm each action's behavior
  4. New self-created skills appear in the next session's skill index with the `Self-created` trust tier; agents can load them via the existing progressive-disclosure path (names+summaries → on-demand full content)
**Plans:** TBD (estimated 3 plans — trigger heuristic detection, SKILL.md scaffold + agentskills.io frontmatter, skill_manage tool + 6 actions)

Plans:
- [ ] TBD (run /gsd-plan-phase 33 to break down)

**Phase directory:** `.planning/phases/33-autonomous-skill-creation/`
